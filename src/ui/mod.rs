// `theme.rs` vive en la raíz del crate (`crate::theme`), no acá, porque
// `bidi.rs` y `theme.rs` se referencian entre sí por esa ruta (son tu
// código original, sin tocar). Este `pub use` deja que el resto de los
// módulos de acá sigan escribiendo `crate::ui::theme::...` sin cambios.
pub(crate) use crate::theme;

pub mod auth;
pub mod call;
pub mod chat;
pub mod dm;
pub mod home;
pub mod profile_popup;
pub mod server;
pub mod settings;
pub mod shell;
