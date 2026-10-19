//! What makes this build Nautilus: the one constant the shared shell
//! takes its product strings and icon from.

use osmena_shell::osmena_core::Brand;

pub static BRAND: Brand = Brand {
    name: "Nautilus",
    binary: "nautilus",
    data_dir: "Nautilus",
    homepage: "https://www.wikipedia.org/",
    icon_png: include_bytes!("../../../assets/icons/nautilus-256.png"),
};
