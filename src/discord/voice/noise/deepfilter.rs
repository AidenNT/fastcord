//! Supresión de ruido con DeepFilterNet (alternativa a RNNoise).
//!
//! Con `--features deepfilternet-ort` usa `vendor/deepfilter-rt` sobre ONNX
//! Runtime (streaming frame a frame, con el estado de las GRU). Necesita en
//! ejecución la biblioteca de ONNX Runtime (`onnxruntime.dll` /
//! `libonnxruntime.so` / `libonnxruntime.dylib`, ver `resolve_ort_lib` en el
//! crate) y una carpeta de modelo. Sin el feature, este módulo es un stub que
//! nunca procesa y la cadena sigue con RNNoise.
//!
//! El modelo se busca (`find_model_dir`) y, si no está en el disco, se descarga
//! una sola vez a la carpeta de caché de ecord (`obtain_model`). Todo eso, y la
//! carga, ocurre en un hilo aparte la primera vez que se pide: mientras no esté
//! listo (o si algo falla), `process` devuelve `None` y el llamador usa RNNoise,
//! así el hilo de audio nunca se bloquea. El estado (descargando / cargando /
//! listo / falló) se publica en `clean_mic::deepfilter_status` para el popup, y
//! los detalles de los errores salen con el log de depuración ("deepfilter").
//!
//! Variables de entorno:
//! * `ECORD_DF_MODEL_DIR`: carpeta de un modelo ya armado (con `config.ini` y los `.onnx`).
//! * `ECORD_DF_MODEL_URL`: URL base alternativa para la descarga (se prueba primero).
//! * `ECORD_DF_NO_DOWNLOAD`: si existe, nunca descarga.

#[cfg(feature = "deepfilternet-ort")]
mod imp {
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use deepfilter_rt::{DeepFilterProcessor, HOP_SIZE, SessionMode};

    use super::super::chain::FRAME;
    use crate::discord::clean_mic::{DeepFilterStatus, set_deepfilter_status};

    // La cadena trabaja en frames de 10 ms a 48 kHz, igual que el modelo.
    const _: () = assert!(HOP_SIZE == FRAME);

    /// Carpetas de modelo que se buscan en disco, de mejor a más liviano.
    const MODEL_NAMES: [&str; 3] = ["dfn3_h0", "dfn3", "dfn3_ll"];

    /// Archivos de un modelo en el formato "split streaming" de deepfilter-rt.
    const ONNX_MARKERS: [&str; 4] = [
        "enc_conv_streaming.onnx",
        "combined_streaming.onnx",
        "combined.onnx",
        "enc.onnx",
    ];

    // ---- descarga ---------------------------------------------------------

    /// Qué se baja si no hay ningún modelo en disco: DeepFilterNet3 (`dfn3`),
    /// split streaming, del repo de deepfilter-rt.
    const DOWNLOAD_DIR_NAME: &str = "dfn3";
    const DOWNLOAD_FILES: [&str; 5] = [
        "config.ini",
        "enc_conv_streaming.onnx",
        "enc_gru_streaming.onnx",
        "erb_dec_streaming.onnx",
        "df_dec_streaming.onnx",
    ];
    /// Se prueba primero el commit fijado (reproducible) y después `master`.
    const DOWNLOAD_BASE_URLS: [&str; 2] = [
        "https://raw.githubusercontent.com/shimondoodkin/deepfilter-rt/f1a1b27be767a5e62ccc8625017f1737c33a3bf9/models/dfn3",
        "https://raw.githubusercontent.com/shimondoodkin/deepfilter-rt/master/models/dfn3",
    ];
    /// Después de una descarga fallida no se vuelve a intentar hasta pasado este tiempo.
    const RETRY_AFTER: Duration = Duration::from_secs(300);

    /// Una sola descarga a la vez; las demás instancias esperan y encuentran el modelo ya puesto.
    static DOWNLOAD_LOCK: Mutex<()> = Mutex::new(());
    static LAST_FAILURE: Mutex<Option<Instant>> = Mutex::new(None);

    struct Engine {
        df: DeepFilterProcessor,
        /// Entrada original retrasada lo mismo que la salida del modelo; sirve
        /// para limitar la atenuación mezclando un poco de señal sin tratar.
        dry: VecDeque<f32>,
        input: [f32; HOP_SIZE],
        output: [f32; HOP_SIZE],
    }

    pub struct DeepFilter {
        slot: Arc<Mutex<Option<Engine>>>,
        loading: bool,
        /// Un error de inferencia descarta el motor y no se reintenta.
        failed: bool,
    }

    impl DeepFilter {
        pub fn new() -> Self {
            Self {
                slot: Arc::new(Mutex::new(None)),
                loading: false,
                failed: false,
            }
        }

        fn start_loading(&mut self) {
            self.loading = true;
            let slot = Arc::clone(&self.slot);
            std::thread::Builder::new()
                .name("deepfilternet-load".into())
                .spawn(move || match load_engine() {
                    Ok(engine) => {
                        if let Ok(mut guard) = slot.lock() {
                            *guard = Some(engine);
                        }
                        set_deepfilter_status(DeepFilterStatus::Ready);
                    }
                    Err(error) => {
                        set_deepfilter_status(DeepFilterStatus::Failed);
                        crate::logging::debug("deepfilter", format!("no se pudo cargar DeepFilterNet (ONNX): {error}"));
                    }
                })
                .ok();
        }

        /// Procesa un frame de 10 ms a 48 kHz mono.
        ///
        /// Entrada: muestras en escala i16 (`x * 32768`). Salida si devuelve
        /// `Some`: muestras ya en -1..1 y una estimación 0..1 de voz (derivada
        /// del SNR local que reporta el modelo). Con `None` el buffer queda
        /// intacto.
        pub fn process(&mut self, buf: &mut [f32], atten_db: f32) -> Option<f32> {
            if self.failed {
                return None;
            }
            if !self.loading {
                self.start_loading();
            }
            // `try_lock`: nunca esperamos al hilo de carga desde el audio.
            let mut guard = self.slot.try_lock().ok()?;
            let engine = guard.as_mut()?;
            if buf.len() != FRAME {
                return None;
            }

            for (dst, src) in engine.input.iter_mut().zip(buf.iter()) {
                *dst = *src / 32768.0;
            }
            let failure = match engine.df.process_frame(&engine.input, &mut engine.output) {
                Ok(()) => None,
                Err(error) => Some(error.to_string()),
            };
            if let Some(error) = failure {
                crate::logging::debug("deepfilter", format!("falló la inferencia, se vuelve a RNNoise: {error}"));
                set_deepfilter_status(DeepFilterStatus::Failed);
                *guard = None;
                self.failed = true;
                return None;
            }
            if engine.output.iter().any(|x| !x.is_finite()) {
                // Estado corrupto: se reinicia el modelo y este frame va por RNNoise.
                engine.df.reset();
                engine.dry.clear();
                let delay = engine.df.delay_samples();
                engine.dry.resize(delay, 0.0);
                return None;
            }

            // Límite de atenuación (como `atten_lim` de libDF): `lim` es cuánta
            // señal sin tratar se deja pasar, 10^(-dB/20).
            let lim = 10f32.powf(-atten_db.max(0.0) / 20.0).clamp(0.0, 1.0);
            for i in 0..FRAME {
                engine.dry.push_back(engine.input[i]);
                let dry = engine.dry.pop_front().unwrap_or(0.0);
                buf[i] = engine.output[i] * (1.0 - lim) + dry * lim;
            }

            // lsnr ~ -15..35 dB: ruido puro .. voz limpia.
            Some(((engine.df.last_lsnr() + 5.0) / 25.0).clamp(0.0, 1.0))
        }
    }

    // ---- búsqueda del modelo en disco -------------------------------------

    /// Carpeta de caché donde se guarda el modelo descargado.
    fn model_cache_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "ecord").map(|dirs| dirs.cache_dir().join("models"))
    }

    /// Una carpeta sirve si tiene `config.ini` y al menos un `.onnx` del modelo.
    fn is_model(dir: &Path) -> bool {
        dir.join("config.ini").is_file() && ONNX_MARKERS.iter().any(|name| dir.join(name).is_file())
    }

    /// Carpeta con `config.ini` + los `.onnx` del modelo, si ya está en disco.
    fn find_model_dir() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("ECORD_DF_MODEL_DIR") {
            let dir = PathBuf::from(dir);
            if is_model(&dir) {
                return Some(dir);
            }
        }

        let mut roots: Vec<PathBuf> = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                roots.push(parent.join("models"));
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            roots.push(cwd.join("models"));
        }
        if let Some(cache) = model_cache_dir() {
            roots.push(cache);
        }
        roots.push(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/vendor/deepfilter-rt/models")));

        roots
            .iter()
            .flat_map(|root| MODEL_NAMES.iter().map(move |name| root.join(name)))
            .find(|dir| is_model(dir))
    }

    // ---- descarga del modelo ----------------------------------------------

    /// Devuelve la carpeta del modelo, bajándolo si hace falta. Corre en el hilo de carga.
    fn obtain_model() -> Result<PathBuf, String> {
        let _guard = DOWNLOAD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Otra instancia pudo haberlo bajado mientras esperábamos el candado.
        if let Some(dir) = find_model_dir() {
            return Ok(dir);
        }
        if std::env::var_os("ECORD_DF_NO_DOWNLOAD").is_some() {
            return Err("no hay modelo en disco y la descarga está desactivada (ECORD_DF_NO_DOWNLOAD)".to_string());
        }
        let last_failure = *LAST_FAILURE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(at) = last_failure {
            if at.elapsed() < RETRY_AFTER {
                return Err("la descarga del modelo falló hace poco; se reintenta más tarde".to_string());
            }
        }

        set_deepfilter_status(DeepFilterStatus::DownloadingModel);
        crate::logging::debug("deepfilter", "no hay modelo en disco, descargando DeepFilterNet3 (unos MB, una sola vez)");
        match download_model() {
            Ok(dir) => {
                crate::logging::debug("deepfilter", format!("modelo descargado en {}", dir.display()));
                Ok(dir)
            }
            Err(error) => {
                *LAST_FAILURE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
                Err(format!("no se pudo descargar el modelo: {error}"))
            }
        }
    }

    fn download_base_urls() -> Vec<String> {
        let mut urls = Vec::new();
        if let Some(custom) = std::env::var_os("ECORD_DF_MODEL_URL") {
            let custom = custom.to_string_lossy().trim().trim_end_matches('/').to_string();
            if !custom.is_empty() {
                urls.push(custom);
            }
        }
        urls.extend(DOWNLOAD_BASE_URLS.iter().map(|url| url.to_string()));
        urls
    }

    /// Baja todos los archivos a `<caché>/models/dfn3.part` y recién al terminar
    /// renombra la carpeta a `dfn3`: nunca queda un modelo a medias.
    fn download_model() -> Result<PathBuf, String> {
        let root = model_cache_dir().ok_or_else(|| "no se pudo resolver la carpeta de caché".to_string())?;
        std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
        let final_dir = root.join(DOWNLOAD_DIR_NAME);
        let part_dir = root.join(format!("{DOWNLOAD_DIR_NAME}.part"));

        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| e.to_string())?;

        let mut last_error = "sin URLs de descarga".to_string();
        for base in download_base_urls() {
            let _ = std::fs::remove_dir_all(&part_dir);
            std::fs::create_dir_all(&part_dir).map_err(|e| format!("{}: {e}", part_dir.display()))?;
            match download_files(&client, &base, &part_dir) {
                Ok(()) => {
                    let _ = std::fs::remove_dir_all(&final_dir);
                    std::fs::rename(&part_dir, &final_dir)
                        .map_err(|e| format!("no se pudo mover {} a {}: {e}", part_dir.display(), final_dir.display()))?;
                    return Ok(final_dir);
                }
                Err(error) => {
                    crate::logging::debug("deepfilter", format!("descarga desde {base} falló: {error}"));
                    last_error = error;
                    let _ = std::fs::remove_dir_all(&part_dir);
                }
            }
        }
        Err(last_error)
    }

    fn download_files(client: &reqwest::blocking::Client, base: &str, dir: &Path) -> Result<(), String> {
        let total = DOWNLOAD_FILES.len() as u8;
        crate::discord::clean_mic::set_deepfilter_download_progress(0, total);
        for (index, name) in DOWNLOAD_FILES.into_iter().enumerate() {
            crate::discord::clean_mic::set_deepfilter_download_progress(index as u8, total);
            let url = format!("{base}/{name}");
            let _who = discord_client_rest::netlog::origin("deepfilter (bajar modelo de audio)");
            let trace = crate::support::nettrace::begin("GET", &url, "deepfilter");
            let sent = client.get(&url).send();
            crate::support::nettrace::observe_blocking(&trace, &sent);
            let bytes = sent
                .and_then(|response| response.error_for_status())
                .and_then(|response| response.bytes())
                .map_err(|e| format!("{url}: {e}"))?;
            if bytes.is_empty() {
                return Err(format!("{url}: respuesta vacía"));
            }
            // Un .onnx real pesa cientos de KB; si es chico y empieza así, es un puntero de Git LFS.
            if name.ends_with(".onnx") && (bytes.len() < 4096 || bytes.starts_with(b"version https://git-lfs")) {
                return Err(format!("{url}: no es un modelo ONNX ({} bytes, ¿puntero de Git LFS?)", bytes.len()));
            }
            std::fs::write(dir.join(name), &bytes).map_err(|e| format!("{}: {e}", dir.join(name).display()))?;
        }
        Ok(())
    }

    // ---- carga ------------------------------------------------------------

    fn load_engine() -> Result<Engine, String> {
        let dir = match find_model_dir() {
            Some(dir) => dir,
            None => obtain_model()?,
        };
        set_deepfilter_status(DeepFilterStatus::Loading);
        // 1 hilo de ONNX Runtime: menos variación de latencia y deja CPU al resto del cliente.
        let mut df = DeepFilterProcessor::with_mode(&dir, SessionMode::Auto, Some(1)).map_err(|e| e.to_string())?;
        // La primera inferencia es lenta; se hace acá, no en el hilo de audio.
        df.warmup().map_err(|e| e.to_string())?;

        let delay = df.delay_samples();
        let mut dry = VecDeque::with_capacity(delay + HOP_SIZE);
        dry.resize(delay, 0.0);
        crate::logging::debug(
            "deepfilter",
            format!(
                "DeepFilterNet listo: {} [{}] desde {} (retardo {} ms)",
                df.variant().name(),
                df.inference_mode_name(),
                dir.display(),
                delay * 1000 / deepfilter_rt::SAMPLE_RATE,
            ),
        );
        Ok(Engine {
            df,
            dry,
            input: [0.0; HOP_SIZE],
            output: [0.0; HOP_SIZE],
        })
    }
}

#[cfg(not(feature = "deepfilternet-ort"))]
mod imp {
    pub struct DeepFilter;

    impl DeepFilter {
        pub fn new() -> Self {
            Self
        }

        pub fn process(&mut self, _buf: &mut [f32], _atten_db: f32) -> Option<f32> {
            None
        }
    }
}

pub use imp::DeepFilter;
