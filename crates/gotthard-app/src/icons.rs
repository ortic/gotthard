//! Gotthard's icon set: 16×16 line icons drawn as SVG, white, and tinted
//! where they're shown, so they're crisp at any size and in both themes.

use std::borrow::Cow;
use std::sync::OnceLock;

use egui::load::Bytes;
use egui::{Color32, Image, ImageSource, Response, Ui, vec2};

macro_rules! icons {
    ($($name:ident => $body:expr),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Icon { $($name),* }

        impl Icon {
            pub const ALL: &[Icon] = &[$(Icon::$name),*];

            fn body(self) -> &'static str {
                match self { $(Icon::$name => $body),* }
            }

            fn name(self) -> &'static str {
                match self { $(Icon::$name => stringify!($name)),* }
            }
        }
    };
}

/// The outline of a sheet of paper, shared by the file icons.
macro_rules! sheet {
    ($inner:expr) => {
        concat!(
            r#"<path d="M4.5 2h4.6l3.4 3.4v7.1a1.5 1.5 0 0 1-1.5 1.5H4.5A1.5 1.5 0 0 1 3 12.5v-9A1.5 1.5 0 0 1 4.5 2z" fill="white" fill-opacity=".14"/><path d="M9 2.2v3.3h3.3"/>"#,
            $inner
        )
    };
}

macro_rules! folder {
    ($inner:expr) => {
        concat!(
            r#"<path d="M1.8 4.3A1.3 1.3 0 0 1 3.1 3h2.7c.4 0 .8.2 1 .5l.9 1.2h5.2a1.3 1.3 0 0 1 1.3 1.3v5.7a1.3 1.3 0 0 1-1.3 1.3H3.1a1.3 1.3 0 0 1-1.3-1.3z" fill="white" fill-opacity=".28"/>"#,
            $inner
        )
    };
}

icons! {
    // The views.
    Sync => r#"<path d="M2.5 7.5a5.5 5.5 0 0 1 9.6-3.2l1.4 1.5"/><path d="M13.5 2.6v3.2h-3.2"/><path d="M13.5 8.5a5.5 5.5 0 0 1-9.6 3.2l-1.4-1.5"/><path d="M2.5 13.4v-3.2h3.2"/>"#,
    Backup => r#"<path d="M3.4 9 5 3.6c.1-.4.5-.6.9-.6h4.2c.4 0 .8.2.9.6L12.6 9"/><rect x="2" y="9" width="12" height="4.5" rx="1.3" fill="white" fill-opacity=".14"/><path d="M11.4 11.25h.01M9.2 11.25h.01"/>"#,
    Shield => r#"<path d="M8 1.8 13 3.7v4.1c0 3-2 5.2-5 6.4-3-1.2-5-3.4-5-6.4V3.7z" fill="white" fill-opacity=".14"/><path d="m5.7 8 1.6 1.6 3-3.1"/>"#,
    Panes => r#"<rect x="2" y="2.5" width="12" height="11" rx="1.6" fill="white" fill-opacity=".14"/><path d="M8 2.5v11M4.2 5.5h1.6M4.2 8h1.6M10.2 5.5h1.6M10.2 8h1.6"/>"#,
    Merge => r#"<path d="M2.5 3.5h2.2c1.2 0 2.2.7 2.7 1.8l.5 1.2c.4 1 1.4 1.5 2.4 1.5h3.2"/><path d="M2.5 12.5h2.2c1.2 0 2.2-.7 2.7-1.8"/><path d="m11 5.5 2.5 2.5L11 10.5"/>"#,
    Duplicates => r#"<rect x="5.5" y="5.5" width="8.5" height="8.5" rx="1.6" fill="white" fill-opacity=".14"/><path d="M10.5 3.2V3A1.5 1.5 0 0 0 9 2H3.5A1.5 1.5 0 0 0 2 3.500V9a1.5 1.500 0 0 0 1.500 1.500h.2"/>"#,
    Similar => r#"<path d="M2.500 6.300c1.800-2 3.700-2 5.500 0s3.700 2 5.500 0"/><path d="M2.500 10.700c1.800-2 3.700-2 5.500 0s3.700 2 5.500 0"/>"#,
    Rename => r#"<path d="M10.600 2.700 13.300 5.400 5.600 13.100l-3.200.5.500-3.200z" fill="white" fill-opacity=".14"/><path d="m9 4.300 2.700 2.700"/>"#,
    Settings => r#"<path d="M2.500 4.500h5M11.500 4.500h2M2.500 11.500h2M8.500 11.500h5"/><circle cx="9.500" cy="4.500" r="1.800"/><circle cx="6.500" cy="11.500" r="1.800"/>"#,

    // Files and places.
    Folder => folder!(""),
    FolderPlus => folder!(r#"<path d="M8 7.300v3.400M6.300 9h3.400"/>"#),
    File => sheet!(""),
    FileText => sheet!(r#"<path d="M5.500 8.300h5M5.500 10.800h3.500"/>"#),
    FileImage => sheet!(r#"<path d="m4.800 11.800 2-2.600 1.500 1.600 1-1 1.900 2"/><path d="M6.200 6.600h.01"/>"#),
    FileMedia => sheet!(r#"<path d="m6.700 7.800 3.200 1.900-3.200 1.900z" fill="white"/>"#),
    FileArchive => sheet!(r#"<path d="M6.500 3.500v1M6.500 6v1M6.500 8.500v1"/><rect x="5.500" y="10.500" width="2" height="1.800" rx=".4"/>"#),
    FileCode => sheet!(r#"<path d="m6.600 8.200-1.500 1.500 1.500 1.500M9.400 8.200l1.500 1.500-1.500 1.500"/>"#),
    Drive => r#"<rect x="2" y="4" width="12" height="8" rx="1.600" fill="white" fill-opacity=".14"/><path d="M4.500 9.500h.01M6.700 9.500h.01M2 7.500h12"/>"#,
    Computer => r#"<rect x="2" y="3" width="12" height="8" rx="1.300" fill="white" fill-opacity=".14"/><path d="M5.500 13.500h5M8 11v2.500"/>"#,
    Home => r#"<path d="M2.500 7.600 8 2.800l5.500 4.800"/><path d="M4 6.500v6.700h8V6.500" fill="white" fill-opacity=".14"/><path d="M6.800 13.200V9.800h2.400v3.400"/>"#,

    // Directions and actions.
    ArrowRight => r#"<path d="M2.500 8h10.500"/><path d="m9.300 4.300 3.700 3.700-3.700 3.700"/>"#,
    ArrowLeft => r#"<path d="M13.500 8H3"/><path d="M6.700 4.300 3 8l3.700 3.700"/>"#,
    ArrowUp => r#"<path d="M8 13.500V3"/><path d="M4.300 6.700 8 3l3.700 3.700"/>"#,
    ArrowBoth => r#"<path d="M2.500 5.500h10.500M10.700 3.200 13 5.500l-2.300 2.300"/><path d="M13.500 10.500H3M5.300 8.200 3 10.500l2.300 2.300"/>"#,
    Trash => r#"<path d="M2.800 4.500h10.400M6 4.500V3.300c0-.4.300-.8.800-.8h2.400c.500 0 .8.400.8.800v1.200"/><path d="m4 4.500.600 8.100c0 .500.500.900 1 .900h4.800c.500 0 1-.400 1-.900l.600-8.100" fill="white" fill-opacity=".14"/><path d="M6.700 7.200v3.600M9.300 7.200v3.600"/>"#,
    Equal => r#"<path d="M4 6h8M4 10h8"/>"#,
    Alert => r#"<path d="M8 2.300 14.200 13H1.800z" fill="white" fill-opacity=".14"/><path d="M8 6.500v3M8 11.300h.01"/>"#,
    Check => r#"<path d="m3 8.500 3.300 3.300L13 4.800"/>"#,
    CheckCircle => r#"<circle cx="8" cy="8" r="6" fill="white" fill-opacity=".14"/><path d="m5.300 8.200 1.900 1.900 3.500-3.700"/>"#,
    Info => r#"<circle cx="8" cy="8" r="6" fill="white" fill-opacity=".14"/><path d="M8 7.300v3.500M8 5.200h.01"/>"#,
    Plus => r#"<path d="M8 3v10M3 8h10"/>"#,
    Refresh => r#"<path d="M13.500 8a5.500 5.500 0 1 1-1.700-4l1.700 1.600"/><path d="M13.500 2.500v3.100h-3.100"/>"#,
    Search => r#"<circle cx="7" cy="7" r="4.500" fill="white" fill-opacity=".14"/><path d="m10.400 10.400 3.100 3.100"/>"#,
    Copy => r#"<rect x="5.500" y="5.500" width="8.500" height="8.500" rx="1.600" fill="white" fill-opacity=".14"/><path d="M10.500 3.200V3A1.500 1.500 0 0 0 9 2H3.500A1.500 1.500 0 0 0 2 3.500V9a1.500 1.500 0 0 0 1.500 1.500h.2"/>"#,
    Undo => r#"<path d="M3 6.500h6.700a3.500 3.500 0 0 1 0 7H6"/><path d="M5.700 3.800 3 6.500l2.700 2.700"/>"#,
    Close => r#"<path d="m4 4 8 8M12 4l-8 8"/>"#,
    ChevronRight => r#"<path d="m6 3.500 4.500 4.500L6 12.500"/>"#,
    ChevronDown => r#"<path d="m3.500 6 4.500 4.500L12.500 6"/>"#,
    Pulse => r#"<path d="M1.800 8.500h2.700l1.500-4 2.600 7.500 1.700-3.500h3.900"/>"#,
    Clock => r#"<circle cx="8" cy="8" r="6" fill="white" fill-opacity=".14"/><path d="M8 4.700V8l2.200 1.500"/>"#,
    Open => r#"<path d="M9 2.800h4.200V7"/><path d="M13 3 7.500 8.500"/><path d="M11.500 9.500v2.700c0 .600-.500 1-1 1H3.800c-.600 0-1-.500-1-1V5.500c0-.600.500-1 1-1h2.700"/>"#,
}

fn documents() -> &'static [(String, String)] {
    static CACHE: OnceLock<Vec<(String, String)>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Icon::ALL
            .iter()
            .map(|icon| {
                (
                    format!("bytes://gotthard-icon/{}.svg", icon.name()),
                    format!(
                        r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="white" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round">{}</svg>"#,
                        icon.body()
                    ),
                )
            })
            .collect()
    })
}

impl Icon {
    pub fn source(self) -> ImageSource<'static> {
        let (uri, svg) = &documents()[self as usize];
        ImageSource::Bytes {
            uri: Cow::Borrowed(uri.as_str()),
            bytes: Bytes::Static(svg.as_bytes()),
        }
    }

    /// The icon `size` points wide and high, in `color`.
    pub fn image(self, size: f32, color: Color32) -> Image<'static> {
        Image::new(self.source())
            .fit_to_exact_size(vec2(size, size))
            .tint(color)
    }

    /// The icon in the color of text, 16 points.
    pub fn show(self, ui: &mut Ui) -> Response {
        let color = ui.visuals().text_color();
        ui.add(self.image(16.0, color))
    }
}

/// Colors of the kinds of files.
pub mod kind {
    use egui::Color32;

    pub const FOLDER: Color32 = Color32::from_rgb(0xe8, 0xb0, 0x3c);
    pub const IMAGE: Color32 = Color32::from_rgb(0x3f, 0xb2, 0x7f);
    pub const MEDIA: Color32 = Color32::from_rgb(0xb0, 0x7c, 0xf0);
    pub const TEXT: Color32 = Color32::from_rgb(0x4f, 0x8f, 0xf7);
    pub const ARCHIVE: Color32 = Color32::from_rgb(0xc9, 0x8a, 0x5a);
    pub const CODE: Color32 = Color32::from_rgb(0x2f, 0xb5, 0xc4);
    pub const PDF: Color32 = Color32::from_rgb(0xe5, 0x5c, 0x5c);
    pub const OTHER: Color32 = Color32::from_rgb(0x8d, 0x95, 0xa3);
}

/// The icon and color for a file of this name, by its extension.
pub fn for_file(name: &str) -> (Icon, Color32) {
    let extension = match name.rfind('.') {
        Some(dot) if dot > 0 => name[dot + 1..].to_ascii_lowercase(),
        _ => String::new(),
    };
    match extension.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tif" | "tiff" | "webp" | "heic" | "raw"
        | "cr2" | "nef" | "arw" | "dng" | "svg" | "ico" | "psd" => (Icon::FileImage, kind::IMAGE),
        "mp4" | "mov" | "avi" | "mkv" | "wmv" | "webm" | "m4v" | "mp3" | "wav" | "flac" | "aac"
        | "ogg" | "m4a" | "wma" => (Icon::FileMedia, kind::MEDIA),
        "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "iso" | "cab" => {
            (Icon::FileArchive, kind::ARCHIVE)
        }
        "pdf" => (Icon::FileText, kind::PDF),
        "txt" | "md" | "doc" | "docx" | "odt" | "rtf" | "xls" | "xlsx" | "ods" | "csv" | "ppt"
        | "pptx" | "odp" | "pages" | "numbers" | "key" | "log" => (Icon::FileText, kind::TEXT),
        "rs" | "py" | "js" | "ts" | "html" | "css" | "json" | "xml" | "yml" | "yaml" | "toml"
        | "c" | "cpp" | "h" | "cs" | "java" | "php" | "sql" | "sh" | "bat" | "ps1" | "ini" => {
            (Icon::FileCode, kind::CODE)
        }
        _ => (Icon::File, kind::OTHER),
    }
}

/// The icon and color of a file or a folder.
pub fn for_entry(name: &str, is_dir: bool) -> (Icon, Color32) {
    if is_dir {
        (Icon::Folder, kind::FOLDER)
    } else {
        for_file(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_extension() {
        assert_eq!(for_file("IMG_0101.JPG").0, Icon::FileImage);
        assert_eq!(for_file("notes.txt").0, Icon::FileText);
        assert_eq!(for_file("backup.tar.gz").0, Icon::FileArchive);
        assert_eq!(for_file(".profile").0, Icon::File);
        assert_eq!(for_entry("2024", true).0, Icon::Folder);
    }

    #[test]
    fn every_icon_is_a_valid_svg() {
        for (_, svg) in documents() {
            assert_eq!(svg.matches("<svg").count(), 1);
            assert!(!svg.contains("$"), "{svg}");
            // Every element but the root closes itself.
            assert_eq!(
                svg.matches('<').count(),
                svg.matches("/>").count() + 2,
                "{svg}"
            );
        }
    }
}
