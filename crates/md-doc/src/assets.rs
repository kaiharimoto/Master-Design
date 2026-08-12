//! Getting a picture into a project.
//!
//! Assets are **content-addressed**: a file is named after a hash of its bytes, so
//! importing the same photograph twice produces one file, and a file's name never changes
//! once written. Both properties matter more here than they would elsewhere.
//!
//! One file per distinct image, because a design iterates — the same logo gets dropped in
//! eleven times over an afternoon — and eleven copies of it is what makes a project
//! directory unusable to send to someone.
//!
//! Names that never change, because a project travels through git and is edited by two
//! parties at once. `hero.png` meaning different bytes in two branches is a merge conflict
//! in a binary file, which is a conflict nobody can resolve. `a3f1c92e.png` either matches
//! or is a different asset, and both are true everywhere.
//!
//! The original filename is not thrown away — it goes on the node as its name, so the
//! layer list says "hero.png" rather than a hash. The hash is the storage key, not the
//! label.

use crate::error::{DocError, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Where assets live inside a project.
pub const ASSETS_DIR: &str = "assets";

/// Formats a browser will display and this module can measure.
///
/// Deliberately a list rather than "anything with an extension": an exported page has to
/// render in a browser, so importing a TIFF would produce a design that looks right in
/// the editor and is blank on the web.
pub const SUPPORTED: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "avif"];

/// An imported asset.
#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    /// Filename inside `assets/`, which is what a node's `asset` field holds.
    pub name: String,
    /// What the file was called when it was imported, for naming the layer.
    pub original_name: String,
    /// Intrinsic size in pixels, where it could be determined.
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub bytes: usize,
    /// False when the identical file was already in the project.
    pub written: bool,
}

impl Asset {
    /// The size to give a node placed from this asset, fitted into a box.
    ///
    /// An 8000-pixel-wide photograph dropped onto a 1440 page should not arrive six times
    /// wider than the artboard, and a 16-pixel icon should not arrive invisible. Both are
    /// scaled to sit inside `max_side` while keeping their proportions.
    pub fn placement_size(&self, max_side: f64) -> (f64, f64) {
        let (w, h) = match (self.width, self.height) {
            (Some(w), Some(h)) if w > 0.0 && h > 0.0 => (w, h),
            // An SVG with no intrinsic size, or a format whose header would not parse.
            // A square is a better guess than nothing, and the user can resize it.
            _ => return (max_side, max_side),
        };

        let scale = (max_side / w).min(max_side / h).min(1.0);
        (w * scale, h * scale)
    }
}

/// Copy a file into a project's `assets/`, returning what to point a node at.
pub fn import(project_dir: &Path, source: &Path) -> Result<Asset> {
    let bytes = fs::read(source)
        .map_err(|e| DocError::Io(format!("could not read {}: {e}", source.display())))?;

    let original_name = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image")
        .to_string();

    import_bytes(project_dir, &original_name, &bytes)
}

/// The same, for bytes that never came from a file — a paste, or a drop from a browser.
pub fn import_bytes(project_dir: &Path, original_name: &str, bytes: &[u8]) -> Result<Asset> {
    let extension = extension_of(original_name, bytes).ok_or_else(|| {
        DocError::Io(format!(
            "{original_name} is not an image format a browser can display — supported: {}",
            SUPPORTED.join(", ")
        ))
    })?;

    let name = format!("{}.{extension}", short_hash(bytes));
    let dir = project_dir.join(ASSETS_DIR);
    fs::create_dir_all(&dir)?;

    let path = dir.join(&name);
    // Content addressing makes this safe: a file with this name has these bytes, so
    // rewriting it would only be a slower way of leaving it alone.
    let written = !path.exists();
    if written {
        fs::write(&path, bytes)
            .map_err(|e| DocError::Io(format!("could not write {}: {e}", path.display())))?;
    }

    let (width, height) = measure(bytes);

    Ok(Asset {
        name,
        original_name: original_name.to_string(),
        width,
        height,
        bytes: bytes.len(),
        written,
    })
}

/// Every asset a project has, in a stable order.
pub fn list(project_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(project_dir.join(ASSETS_DIR)) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .filter(|name| !name.starts_with('.'))
        .collect();
    out.sort();
    out
}

/// The absolute path of an asset, for a caller that needs to hand it to something else.
///
/// Refuses a name that would escape the assets directory. A node's `asset` field is
/// ordinary text in a document that may have been written by a model or edited by hand,
/// so `../../.ssh/id_rsa` has to be a rejection rather than a path.
pub fn path_of(project_dir: &Path, name: &str) -> Result<PathBuf> {
    if name.is_empty()
        || name.contains("..")
        || name.contains('/')
        || name.contains('\\')
        || name.starts_with('.')
    {
        return Err(DocError::Io(format!(
            "{name:?} is not a valid asset name — assets are plain filenames inside the \
             project's assets/ folder"
        )));
    }
    Ok(project_dir.join(ASSETS_DIR).join(name))
}

/// A short, stable name for these bytes.
///
/// FNV-1a over the content, widened with the length. Not a cryptographic hash and not
/// pretending to be one: the job is to give the same file the same name and different
/// files different names, and 64 bits of it makes an accidental collision in a design
/// project's worth of images vanishingly unlikely. A collision here would be visible
/// immediately — the wrong picture — rather than exploitable.
fn short_hash(bytes: &[u8]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash ^= bytes.len() as u64;
    hash = hash.wrapping_mul(PRIME);

    format!("{hash:016x}")
}

/// Work out the format, preferring what the bytes say over what the name claims.
///
/// A file called `logo.png` that is really a JPEG is common enough — people rename
/// things — and a browser goes by the bytes, so this does too.
fn extension_of(name: &str, bytes: &[u8]) -> Option<&'static str> {
    if let Some(sniffed) = sniff(bytes) {
        return Some(sniffed);
    }

    let claimed = name.rsplit('.').next()?.to_ascii_lowercase();
    SUPPORTED.iter().copied().find(|s| *s == claimed).map(|s| {
        // One spelling per format on disk, so `photo.JPEG` and `photo.jpg` with identical
        // bytes do not become two assets.
        if s == "jpeg" {
            "jpg"
        } else {
            s
        }
    })
}

fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some("png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("jpg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    if bytes.len() > 12 && &bytes[4..8] == b"ftyp" && &bytes[8..12] == b"avif" {
        return Some("avif");
    }
    // SVG is text and may open with a comment, an XML declaration or the root element, so
    // there is no magic number — look for the tag in the first stretch of the file.
    let head = &bytes[..bytes.len().min(1024)];
    if let Ok(text) = std::str::from_utf8(head) {
        if text.contains("<svg") {
            return Some("svg");
        }
    }
    None
}

/// Read the intrinsic size out of an image's header.
fn measure(bytes: &[u8]) -> (Option<f64>, Option<f64>) {
    if let Ok(size) = imagesize::blob_size(bytes) {
        return (Some(size.width as f64), Some(size.height as f64));
    }
    // An SVG without width and height attributes genuinely has no intrinsic size; a
    // caller places it at a default rather than at zero.
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("md-doc-assets")
            .join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A real 2×3 PNG, so the header parser has something true to read.
    fn png_2x3() -> Vec<u8> {
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        out.extend_from_slice(&13u32.to_be_bytes());
        out.extend_from_slice(b"IHDR");
        out.extend_from_slice(&2u32.to_be_bytes());
        out.extend_from_slice(&3u32.to_be_bytes());
        out.extend_from_slice(&[8, 6, 0, 0, 0]);
        out.extend_from_slice(&[0, 0, 0, 0]); // CRC — no reader here checks it
        out
    }

    #[test]
    fn an_image_lands_in_the_assets_folder() {
        let dir = tmpdir("import");
        let asset = import_bytes(&dir, "hero.png", &png_2x3()).unwrap();

        assert!(asset.written);
        assert!(asset.name.ends_with(".png"));
        assert!(dir.join(ASSETS_DIR).join(&asset.name).is_file());
        assert_eq!(asset.original_name, "hero.png");
    }

    #[test]
    fn the_intrinsic_size_comes_back_with_it() {
        let dir = tmpdir("measure");
        let asset = import_bytes(&dir, "hero.png", &png_2x3()).unwrap();
        assert_eq!((asset.width, asset.height), (Some(2.0), Some(3.0)));
    }

    #[test]
    fn the_same_image_twice_is_one_file() {
        // The case that makes a project directory unusable otherwise: the same logo
        // dropped in over and over during an afternoon's work.
        let dir = tmpdir("dedupe");
        let first = import_bytes(&dir, "logo.png", &png_2x3()).unwrap();
        let second = import_bytes(&dir, "logo-copy.png", &png_2x3()).unwrap();

        assert_eq!(first.name, second.name);
        assert!(first.written);
        assert!(!second.written, "the identical file was written twice");
        assert_eq!(list(&dir).len(), 1);
    }

    #[test]
    fn different_images_get_different_names() {
        let dir = tmpdir("distinct");
        let mut other = png_2x3();
        other.push(0x42);

        let a = import_bytes(&dir, "a.png", &png_2x3()).unwrap();
        let b = import_bytes(&dir, "b.png", &other).unwrap();
        assert_ne!(a.name, b.name);
        assert_eq!(list(&dir).len(), 2);
    }

    #[test]
    fn the_name_depends_only_on_the_bytes() {
        // What makes a project mergeable: the same asset has the same name in every
        // branch, on every machine, in any order.
        let a = tmpdir("stable-a");
        let b = tmpdir("stable-b");
        assert_eq!(
            import_bytes(&a, "one-name.png", &png_2x3()).unwrap().name,
            import_bytes(&b, "quite-another.png", &png_2x3())
                .unwrap()
                .name
        );
    }

    #[test]
    fn the_bytes_decide_the_format_not_the_filename() {
        let dir = tmpdir("mislabelled");
        // People rename files. A browser goes by the content, so this does too.
        let asset = import_bytes(&dir, "actually-a-png.jpg", &png_2x3()).unwrap();
        assert!(asset.name.ends_with(".png"), "got {}", asset.name);
    }

    #[test]
    fn jpeg_and_jpg_are_one_format() {
        let dir = tmpdir("jpeg");
        let jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0, 16, b'J', b'F', b'I', b'F', 0];
        let asset = import_bytes(&dir, "photo.JPEG", &jpeg).unwrap();
        assert!(asset.name.ends_with(".jpg"), "got {}", asset.name);
    }

    #[test]
    fn an_svg_is_recognised_without_a_magic_number() {
        let dir = tmpdir("svg");
        let svg = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="10" height="20"/>"#;
        let asset = import_bytes(&dir, "icon.svg", svg).unwrap();
        assert!(asset.name.ends_with(".svg"));
    }

    #[test]
    fn a_format_no_browser_can_show_is_refused() {
        // Better than importing it and producing a design that looks right in the editor
        // and is blank on the web.
        let dir = tmpdir("unsupported");
        let err = import_bytes(&dir, "scan.tiff", b"II*\0nonsense").unwrap_err();
        assert!(err.to_string().contains("supported"), "got {err}");
    }

    #[test]
    fn a_placement_size_fits_a_huge_photograph_onto_the_page() {
        let asset = Asset {
            name: "a.png".into(),
            original_name: "a.png".into(),
            width: Some(8000.0),
            height: Some(4000.0),
            bytes: 0,
            written: true,
        };
        let (w, h) = asset.placement_size(600.0);
        assert_eq!((w, h), (600.0, 300.0));
    }

    #[test]
    fn a_small_image_is_placed_at_its_own_size() {
        let asset = Asset {
            name: "a.png".into(),
            original_name: "a.png".into(),
            width: Some(48.0),
            height: Some(48.0),
            bytes: 0,
            written: true,
        };
        assert_eq!(asset.placement_size(600.0), (48.0, 48.0));
    }

    #[test]
    fn an_asset_with_no_intrinsic_size_still_gets_placed() {
        let asset = Asset {
            name: "a.svg".into(),
            original_name: "a.svg".into(),
            width: None,
            height: None,
            bytes: 0,
            written: true,
        };
        assert_eq!(asset.placement_size(300.0), (300.0, 300.0));
    }

    #[test]
    fn an_asset_name_cannot_escape_the_project() {
        // A node's `asset` is ordinary text in a document a model may have written.
        let dir = tmpdir("traversal");
        for bad in [
            "../../.ssh/id_rsa",
            "..",
            "/etc/passwd",
            "sub/dir.png",
            ".hidden",
            "",
        ] {
            assert!(
                path_of(&dir, bad).is_err(),
                "{bad:?} was accepted as an asset name"
            );
        }
        assert!(path_of(&dir, "a3f1c92e.png").is_ok());
    }

    #[test]
    fn importing_a_file_that_is_not_there_says_which_one() {
        let dir = tmpdir("missing");
        let err = import(&dir, Path::new("/nope/absent.png")).unwrap_err();
        assert!(err.to_string().contains("absent.png"), "got {err}");
    }

    #[test]
    fn listing_is_stable_and_skips_hidden_files() {
        let dir = tmpdir("listing");
        import_bytes(&dir, "a.png", &png_2x3()).unwrap();
        let mut other = png_2x3();
        other.push(1);
        import_bytes(&dir, "b.png", &other).unwrap();
        fs::write(dir.join(ASSETS_DIR).join(".DS_Store"), b"junk").unwrap();

        let listed = list(&dir);
        assert_eq!(listed.len(), 2, "got {listed:?}");
        let mut sorted = listed.clone();
        sorted.sort();
        assert_eq!(listed, sorted);
    }
}
