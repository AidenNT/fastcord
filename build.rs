//! Compila `proto/discord_user_settings.proto` (el `PreloadedUserSettings`
//! real de Discord: tema, locale, status, notificaciones, privacidad,
//! favoritos, etc. — el blob que sincroniza entre clientes) a código Rust
//! con `prost`, y lo deja en `OUT_DIR` para que
//! `src/discord/user_settings.rs` lo incluya con `include!(...)`.
//!
//! Usamos `protoc-bin-vendored` para no depender de que el que compila
//! tenga `protoc` instalado en el sistema: es un binario ya armado que
//! viaja adentro del crate (se resuelve como cualquier otra dependencia
//! de Cargo, no hace falta internet aparte del `cargo build` normal).
//! Los `.proto` de `google/protobuf/{wrappers,timestamp}.proto` que
//! importa nuestro proto los resuelve `prost-build` solo: trae copias
//! vendorizadas de los tipos "well-known" y no hace falta agregarlas a
//! mano al include path.

fn main() {
    let proto_file = "proto/discord_user_settings.proto";
    println!("cargo:rerun-if-changed={proto_file}");

    let protoc_path = protoc_bin_vendored::protoc_bin_path()
        .expect("no se pudo ubicar el protoc vendorizado (protoc-bin-vendored)");
    // SAFETY: build.rs corre en un solo hilo, antes de que exista
    // cualquier otro código de la app que pueda leer/escribir el
    // environment al mismo tiempo.
    unsafe {
        std::env::set_var("PROTOC", protoc_path);
    }

    prost_build::Config::new()
        .compile_protos(&[proto_file], &["proto/"])
        .expect("no se pudo compilar proto/discord_user_settings.proto");
}
