// `theme.rs` vive en la raíz del crate (`crate::theme`), no acá, porque
// `bidi.rs` y `theme.rs` se referencian entre sí por esa ruta (son tu
// código original, sin tocar). Este `pub use` deja que el resto de los
// módulos de acá sigan escribiendo `crate::ui::theme::...` sin cambios.
pub(crate) use crate::theme;

pub mod account_settings;
pub mod anim;
pub mod call_bar;
pub mod call_view;
pub mod chat;
pub mod components;
pub mod dm;
pub mod emoji;
pub mod extra;
pub mod forum;
pub mod friends_panel;
pub mod home;
pub mod login;
pub mod markdown;
pub mod media;
pub mod nav;
pub mod notifications;
pub mod overlay;
pub mod profile_popup;
pub mod rail;
pub mod role_popup;
pub mod server;
pub mod settings;
pub mod stream_view;
pub mod topbar;
pub mod video_player;
