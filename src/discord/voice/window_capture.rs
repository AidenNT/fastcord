//! Captura de UNA ventana con Windows Graphics Capture (WGC), solo Windows.
//!
//! Es la API que Microsoft recomienda (la misma que usa OBS en "Windows 10
//! (1903 y posterior)") y reemplaza al `gdigrab title=...` anterior, que:
//!
//! * encontraba la ventana por título una sola vez, así que dos ventanas con
//!   el mismo título eran indistinguibles y un cambio de título la perdía;
//! * copiaba con BitBlt, por lo que los navegadores, juegos y apps con
//!   aceleración por GPU salían en negro;
//! * dependía de lo que hubiera encima de la ventana.
//!
//! Acá la ventana se identifica por su `HWND` y WGC entrega su contenido ya
//! compuesto por DWM (aunque esté tapada por otra ventana). No hay un hilo ni
//! un callback: `poll` saca de la cola el último cuadro disponible y lo copia a
//! un `Video` BGRA de ffmpeg, que es lo que ya sabe escalar `FrameConverter`.
//!
//! WGC solo entrega un cuadro cuando el contenido cambia. Si la ventana está
//! quieta, `latest` sigue devolviendo el último, y quien llama lo reenvía al
//! ritmo de fps para que el codificador y los espectadores no se queden sin
//! video.

use std::ffi::c_void;

use ffmpeg_the_third as ffmpeg;
use ffmpeg::format::Pixel;
use ffmpeg::frame::Video;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow};
use windows::core::Interface;

const PIXEL_FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;
/// Cuadros que WGC deja en cola. Con 2 alcanza: se lee de a uno por vuelta.
const POOL_BUFFERS: i32 = 2;

pub(super) struct WindowCapture {
    hwnd: HWND,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    pool_size: SizeInt32,
    /// Textura de la GPU que se puede leer desde la CPU, con su tamaño.
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    latest: Option<Video>,
}

fn wgc_error(context: &str, error: windows::core::Error) -> String {
    format!("{context}: {error}")
}

impl WindowCapture {
    /// `Err` si WGC no está disponible (Windows anterior a 1903) o la ventana
    /// no se puede capturar; quien llama cae a `gdigrab` en ese caso.
    pub(super) fn new(hwnd: isize) -> Result<Self, String> {
        // SAFETY: el HWND es un entero opaco; `IsWindow` lo valida sin
        // desreferenciarlo.
        let hwnd = HWND(hwnd as *mut c_void);
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return Err("la ventana ya no existe".to_owned());
        }
        if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
            return Err("este Windows no tiene Windows Graphics Capture (hace falta 10 1903+)".to_owned());
        }
        // El hilo del codificador no inicializó WinRT. Si ya estaba
        // inicializado (o con otro modelo) el error es inofensivo.
        // SAFETY: llamada sin punteros.
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };

        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .map_err(|error| wgc_error("interop de captura", error))?;
        // SAFETY: `hwnd` se validó arriba.
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd) }
            .map_err(|error| wgc_error("no se pudo capturar esa ventana", error))?;
        let size = item
            .Size()
            .map_err(|error| wgc_error("tamaño de la ventana", error))?;

        let mut device = None;
        let mut context = None;
        // SAFETY: punteros de salida válidos; sin adaptador explícito.
        unsafe {
            D3D11CreateDevice(
                None::<&IDXGIAdapter>,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(|error| wgc_error("no se pudo crear el dispositivo D3D11", error))?;
        let (Some(device), Some(context)) = (device, context) else {
            return Err("D3D11 no devolvió dispositivo".to_owned());
        };
        let dxgi: IDXGIDevice = device
            .cast()
            .map_err(|error| wgc_error("dispositivo DXGI", error))?;
        // SAFETY: `dxgi` es un dispositivo DXGI válido.
        let winrt_device: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
            .and_then(|inspectable| inspectable.cast())
            .map_err(|error| wgc_error("dispositivo WinRT", error))?;

        // `CreateFreeThreaded`: no hace falta un DispatcherQueue; los cuadros
        // se leen por sondeo con `TryGetNextFrame`.
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_device,
            PIXEL_FORMAT,
            POOL_BUFFERS,
            size,
        )
        .map_err(|error| wgc_error("cola de cuadros", error))?;
        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|error| wgc_error("sesión de captura", error))?;
        let _ = session.SetIsCursorCaptureEnabled(true);
        // Quita el borde amarillo donde Windows lo permite (11+); en el resto
        // falla y se ignora.
        let _ = session.SetIsBorderRequired(false);
        session
            .StartCapture()
            .map_err(|error| wgc_error("no se pudo iniciar la captura", error))?;

        Ok(Self {
            hwnd,
            device,
            context,
            winrt_device,
            pool,
            session,
            pool_size: size,
            staging: None,
            latest: None,
        })
    }

    /// Trae el cuadro nuevo, si lo hay. `Err` cuando la ventana se cerró.
    pub(super) fn poll(&mut self) -> Result<(), String> {
        // SAFETY: ver `new`.
        if !unsafe { IsWindow(Some(self.hwnd)) }.as_bool() {
            return Err("la ventana se cerró".to_owned());
        }
        // Una ventana minimizada no produce cuadros: se sigue mostrando el
        // último hasta que se restaure.
        // SAFETY: ver `new`.
        if unsafe { IsIconic(self.hwnd) }.as_bool() {
            return Ok(());
        }
        // `TryGetNextFrame` falla cuando no hay nada nuevo; no es un error.
        let Ok(frame) = self.pool.TryGetNextFrame() else {
            return Ok(());
        };
        let content = frame
            .ContentSize()
            .map_err(|error| wgc_error("tamaño del cuadro", error))?;
        if content.Width <= 0 || content.Height <= 0 {
            return Ok(());
        }
        // La ventana cambió de tamaño: los próximos cuadros salen con el nuevo.
        if content.Width != self.pool_size.Width || content.Height != self.pool_size.Height {
            self.pool
                .Recreate(&self.winrt_device, PIXEL_FORMAT, POOL_BUFFERS, content)
                .map_err(|error| wgc_error("redimensionando la captura", error))?;
            self.pool_size = content;
        }

        let surface = frame
            .Surface()
            .map_err(|error| wgc_error("superficie del cuadro", error))?;
        let access: IDirect3DDxgiInterfaceAccess = surface
            .cast()
            .map_err(|error| wgc_error("acceso DXGI a la superficie", error))?;
        // SAFETY: la superficie de WGC es una `ID3D11Texture2D`.
        let texture: ID3D11Texture2D = unsafe { access.GetInterface() }
            .map_err(|error| wgc_error("textura del cuadro", error))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` es un destino válido.
        unsafe { texture.GetDesc(&mut desc) };

        let width = (content.Width as u32).min(desc.Width).max(2);
        let height = (content.Height as u32).min(desc.Height).max(2);
        self.ensure_staging(&desc)?;
        let Some((staging, _, _)) = self.staging.as_ref() else {
            return Ok(());
        };

        let needs_new = self
            .latest
            .as_ref()
            .is_none_or(|video| video.width() != width || video.height() != height);
        if needs_new {
            self.latest = Some(Video::new(Pixel::BGRA, width, height));
        }
        let Some(video) = self.latest.as_mut() else {
            return Ok(());
        };

        // SAFETY: `staging` y `texture` son del mismo dispositivo y tienen el
        // mismo tamaño y formato; el mapeo se libera antes de salir.
        unsafe {
            self.context.CopyResource(staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|error| wgc_error("leyendo el cuadro de la GPU", error))?;
            let stride = video.stride(0);
            let row_bytes = width as usize * 4;
            let source = mapped.pData as *const u8;
            let pitch = mapped.RowPitch as usize;
            let target = video.data_mut(0);
            for row in 0..height as usize {
                let from = std::slice::from_raw_parts(source.add(row * pitch), row_bytes);
                target[row * stride..row * stride + row_bytes].copy_from_slice(from);
            }
            self.context.Unmap(staging, 0);
        }
        Ok(())
    }

    /// Último cuadro recibido (todavía ninguno al arrancar).
    pub(super) fn latest(&self) -> Option<&Video> {
        self.latest.as_ref()
    }

    fn ensure_staging(&mut self, source: &D3D11_TEXTURE2D_DESC) -> Result<(), String> {
        if self
            .staging
            .as_ref()
            .is_some_and(|(_, width, height)| *width == source.Width && *height == source.Height)
        {
            return Ok(());
        }
        let mut desc = *source;
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut texture = None;
        // SAFETY: `desc` describe una textura de staging válida.
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut texture)) }
            .map_err(|error| wgc_error("textura de lectura", error))?;
        let texture = texture.ok_or_else(|| "D3D11 no creó la textura de lectura".to_owned())?;
        self.staging = Some((texture, source.Width, source.Height));
        Ok(())
    }
}

impl Drop for WindowCapture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
