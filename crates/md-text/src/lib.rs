//! # md-text
//!
//! How wide is a line of text?
//!
//! Everything in a design tool leans on that answer. Selection handles, auto-layout,
//! the box an animation slides a heading out of, the crop of a screenshot. Until this
//! crate existed the answer was `characters × font_size × 0.55`, which is wrong for every
//! font, wrong by about 40% for a heading in a wide face, and wrong by a great deal more
//! for any script that is not Latin.
//!
//! So this measures for real: it loads actual font files, shapes runs through
//! [`rustybuzz`] — the same HarfBuzz algorithm a browser uses, so kerning pairs and
//! ligatures come out with the same advances — and reports what came back.
//!
//! ## The fonts travel with the tool
//!
//! Measuring correctly is only half of it. The editor and the exported page have to agree,
//! and they only agree if they use the same font. A system font database cannot promise
//! that: Android has no Inter, and a CI runner has whatever its base image installed. So
//! Inter is bundled (see `packages/md-fonts`) and always loaded first, with system fonts
//! behind it for everything else a designer might reach for.
//!
//! The bundled files are `.woff2` because that is what an exported page wants to serve,
//! and one file that serves both purposes cannot drift from itself. Reading one for
//! measurement costs a Brotli decompression at startup, which happens once.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

mod layout;
mod shape;

pub use layout::{Layout, Line};

/// A font as the exporter needs to ship it: the bytes, and what to call the file.
#[derive(Debug, Clone)]
pub struct WebFont {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    /// Filename to write into `dist/fonts/`.
    pub file_name: String,
    /// The original `.woff2`, unchanged. Not the decompressed form: a browser wants the
    /// compressed one, and re-compressing what we decompressed would be lossy work for
    /// no reason.
    pub bytes: Arc<Vec<u8>>,
}

/// What a caller asks to be measured.
///
/// Primitives rather than a `TextGeometry`, so this crate stays underneath `md-doc`
/// rather than beside it — `md-doc` measures text, so it must be able to depend on this.
#[derive(Debug, Clone)]
pub struct Run<'a> {
    pub text: &'a str,
    pub family: &'a str,
    pub size: f64,
    pub weight: u16,
    pub italic: bool,
    /// Extra tracking as a fraction of the size, the unit designers set it in.
    pub letter_spacing: f64,
    /// Multiple of the size.
    pub line_height: f64,
    /// The measure. `Some` wraps; `None` lets the box hug the text.
    pub max_width: Option<f64>,
}

impl Default for Run<'_> {
    fn default() -> Self {
        Run {
            text: "",
            family: "Inter",
            size: 16.0,
            weight: 400,
            italic: false,
            letter_spacing: 0.0,
            line_height: 1.4,
            max_width: None,
        }
    }
}

/// The loaded fonts.
pub struct Fonts {
    db: fontdb::Database,
    /// Keyed by `(lowercased family, weight, italic)`.
    web: BTreeMap<(String, u16, bool), WebFont>,
}

impl std::fmt::Debug for Fonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fonts")
            .field("faces", &self.db.len())
            .field("embeddable", &self.web.len())
            .finish()
    }
}

/// The Latin subsets of Inter, compiled in.
///
/// `include_bytes!` rather than reading `packages/md-fonts` at run time, because a
/// packaged application has no repository around it and "the tool works unless you moved
/// a folder" is not a property worth having.
const BUNDLED: &[(&str, u16, bool, &[u8])] = &[
    (
        "Inter-300.woff2",
        300,
        false,
        include_bytes!("../../../packages/md-fonts/inter/Inter-300.woff2"),
    ),
    (
        "Inter-400.woff2",
        400,
        false,
        include_bytes!("../../../packages/md-fonts/inter/Inter-400.woff2"),
    ),
    (
        "Inter-500.woff2",
        500,
        false,
        include_bytes!("../../../packages/md-fonts/inter/Inter-500.woff2"),
    ),
    (
        "Inter-600.woff2",
        600,
        false,
        include_bytes!("../../../packages/md-fonts/inter/Inter-600.woff2"),
    ),
    (
        "Inter-700.woff2",
        700,
        false,
        include_bytes!("../../../packages/md-fonts/inter/Inter-700.woff2"),
    ),
    (
        "Inter-800.woff2",
        800,
        false,
        include_bytes!("../../../packages/md-fonts/inter/Inter-800.woff2"),
    ),
    (
        "Inter-400-italic.woff2",
        400,
        true,
        include_bytes!("../../../packages/md-fonts/inter/Inter-400-italic.woff2"),
    ),
    (
        "Inter-700-italic.woff2",
        700,
        true,
        include_bytes!("../../../packages/md-fonts/inter/Inter-700-italic.woff2"),
    ),
];

/// The family used when a document names one nothing can supply.
pub const FALLBACK_FAMILY: &str = "Inter";

impl Default for Fonts {
    fn default() -> Self {
        Fonts::bundled()
    }
}

impl Fonts {
    /// Just the fonts that ship with the tool. Deterministic on every machine, which is
    /// what makes a golden test meaningful.
    pub fn bundled() -> Self {
        let mut fonts = Fonts {
            db: fontdb::Database::new(),
            web: BTreeMap::new(),
        };
        for (file_name, weight, italic, bytes) in BUNDLED {
            fonts.add_woff2("Inter", *weight, *italic, file_name, bytes);
        }
        fonts.db.set_sans_serif_family(FALLBACK_FAMILY);
        fonts
    }

    /// …plus whatever the machine has installed.
    ///
    /// Scanning takes long enough to notice, so callers that measure in bursts should do
    /// this once and hold on to the result.
    pub fn with_system_fonts(mut self) -> Self {
        self.db.load_system_fonts();
        self
    }

    /// Load a project's own `fonts/` directory.
    ///
    /// A design that uses a licensed typeface has to be able to carry it, and a font
    /// sitting next to the document is the only version of that which survives being
    /// cloned onto another machine.
    pub fn load_dir(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match path.extension().and_then(|e| e.to_str()) {
                Some("woff2") => {
                    if let Ok(bytes) = std::fs::read(&path) {
                        self.add_unknown_woff2(&path, &bytes);
                    }
                }
                Some("ttf" | "otf" | "ttc" | "otc") => {
                    let _ = self.db.load_font_file(&path);
                }
                _ => {}
            }
        }
    }

    /// Decompress a `.woff2` and register the face, remembering the original bytes so the
    /// exporter can ship them.
    fn add_woff2(&mut self, family: &str, weight: u16, italic: bool, file_name: &str, raw: &[u8]) {
        let Ok(sfnt) = woff2_patched::convert_woff2_to_ttf(&mut std::io::Cursor::new(raw.to_vec()))
        else {
            // A font that will not decompress is a broken build, but refusing to start the
            // application over it would be worse than measuring with one fewer weight.
            return;
        };
        self.db.load_font_data(sfnt);
        self.web.insert(
            (family.to_lowercase(), weight, italic),
            WebFont {
                family: family.to_string(),
                weight,
                italic,
                file_name: file_name.to_string(),
                bytes: Arc::new(raw.to_vec()),
            },
        );
    }

    /// The same, for a file whose family and weight are not known in advance — read them
    /// back out of the decompressed face rather than guessing from the filename.
    fn add_unknown_woff2(&mut self, path: &Path, raw: &[u8]) {
        let Ok(sfnt) = woff2_patched::convert_woff2_to_ttf(&mut std::io::Cursor::new(raw.to_vec()))
        else {
            return;
        };

        let before: Vec<fontdb::ID> = self.db.faces().map(|f| f.id).collect();
        self.db.load_font_data(sfnt);

        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("font.woff2")
            .to_string();

        for face in self.db.faces() {
            if before.contains(&face.id) {
                continue;
            }
            let Some((family, _)) = face.families.first() else {
                continue;
            };
            self.web.insert(
                (
                    family.to_lowercase(),
                    face.weight.0,
                    face.style != fontdb::Style::Normal,
                ),
                WebFont {
                    family: family.clone(),
                    weight: face.weight.0,
                    italic: face.style != fontdb::Style::Normal,
                    file_name: file_name.clone(),
                    bytes: Arc::new(raw.to_vec()),
                },
            );
            break;
        }
    }

    /// The underlying database, for consumers that do their own text handling — `resvg`
    /// takes one directly, and giving it *this* one is what makes a rasterised snapshot
    /// agree with a measured layout.
    pub fn db(&self) -> &fontdb::Database {
        &self.db
    }

    pub fn is_empty(&self) -> bool {
        self.db.is_empty()
    }

    /// Find the face that best matches a request.
    ///
    /// Returns the id together with whether the family asked for was actually available,
    /// so a caller can report a substitution instead of silently laying out in the wrong
    /// typeface.
    pub fn resolve(&self, family: &str, weight: u16, italic: bool) -> Resolved {
        let style = if italic {
            fontdb::Style::Italic
        } else {
            fontdb::Style::Normal
        };

        let wanted = fontdb::Query {
            families: &[fontdb::Family::Name(family)],
            weight: fontdb::Weight(weight),
            stretch: fontdb::Stretch::Normal,
            style,
        };

        if let Some(id) = self.db.query(&wanted) {
            return Resolved {
                id: Some(id),
                substituted: !self.has_family(family),
            };
        }

        // An italic that does not exist is better served by the upright of the right
        // family than by a different family's italic: the shapes are the design, the slant
        // is a variation on it.
        if italic {
            let upright = fontdb::Query {
                style: fontdb::Style::Normal,
                ..wanted
            };
            if let Some(id) = self.db.query(&upright) {
                return Resolved {
                    id: Some(id),
                    substituted: true,
                };
            }
        }

        let fallback = fontdb::Query {
            families: &[
                fontdb::Family::Name(FALLBACK_FAMILY),
                fontdb::Family::SansSerif,
            ],
            ..wanted
        };
        Resolved {
            id: self.db.query(&fallback),
            substituted: true,
        }
    }

    fn has_family(&self, family: &str) -> bool {
        let wanted = family.to_lowercase();
        self.db.faces().any(|f| {
            f.families
                .iter()
                .any(|(name, _)| name.to_lowercase() == wanted)
        })
    }

    /// The file to embed for a family, weight and style, if one can be shipped.
    ///
    /// Only fonts loaded from `.woff2` — the bundled set and a project's own — can be.
    /// A system font is licensed to the person who installed it, not to everyone who
    /// visits their website, so copying one into an export would be the tool quietly
    /// making a licensing decision on the user's behalf.
    pub fn web_font(&self, family: &str, weight: u16, italic: bool) -> Option<&WebFont> {
        let key = family.to_lowercase();
        self.web
            .get(&(key.clone(), weight, italic))
            .or_else(|| {
                // Nearest weight in the same family and style, the way CSS matches.
                self.web
                    .iter()
                    .filter(|((f, _, i), _)| *f == key && *i == italic)
                    .min_by_key(|((_, w, _), _)| w.abs_diff(weight))
                    .map(|(_, font)| font)
            })
            .or_else(|| {
                self.web
                    .iter()
                    .filter(|((f, _, _), _)| *f == key)
                    .min_by_key(|((_, w, _), _)| w.abs_diff(weight))
                    .map(|(_, font)| font)
            })
    }

    /// Every embeddable face of a family, for an exporter writing `@font-face` rules.
    pub fn web_fonts_for(&self, family: &str) -> Vec<&WebFont> {
        let key = family.to_lowercase();
        self.web
            .iter()
            .filter(|((f, _, _), _)| *f == key)
            .map(|(_, font)| font)
            .collect()
    }

    /// Shape and break a run of text.
    pub fn measure(&self, run: &Run<'_>) -> Layout {
        layout::measure(self, run)
    }
}

/// The outcome of asking for a face.
#[derive(Debug, Clone, Copy)]
pub struct Resolved {
    pub id: Option<fontdb::ID>,
    /// The document asked for a family this machine could not supply.
    pub substituted: bool,
}

/// A process-wide font set, loaded once.
///
/// Scanning system fonts is slow enough to be felt, and measurement happens on every
/// bounds calculation, so the alternative — building a database per call — would make
/// selecting a text node visibly laggy.
pub fn shared() -> &'static Fonts {
    static SHARED: std::sync::OnceLock<Fonts> = std::sync::OnceLock::new();
    SHARED.get_or_init(|| Fonts::bundled().with_system_fonts())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_family_is_there_without_touching_the_system() {
        let fonts = Fonts::bundled();
        assert!(!fonts.is_empty());
        let resolved = fonts.resolve("Inter", 400, false);
        assert!(resolved.id.is_some());
        assert!(
            !resolved.substituted,
            "Inter should resolve to Inter, not a substitute"
        );
    }

    #[test]
    fn every_bundled_weight_survived_decompression() {
        // A `.woff2` that fails to decode is skipped rather than fatal, so without this
        // the family could silently arrive with two weights in it.
        let fonts = Fonts::bundled();
        for (_, weight, italic, _) in BUNDLED {
            assert!(
                fonts.web_font("Inter", *weight, *italic).is_some(),
                "Inter {weight}{} did not load",
                if *italic { " italic" } else { "" }
            );
        }
    }

    #[test]
    fn a_font_nobody_has_is_substituted_and_says_so() {
        let fonts = Fonts::bundled();
        let resolved = fonts.resolve("Nonexistent Grotesk", 400, false);
        assert!(
            resolved.id.is_some(),
            "measurement must still produce something"
        );
        assert!(
            resolved.substituted,
            "a substitution has to be reportable, or a design lays out in the wrong \
             typeface with nothing said"
        );
    }

    #[test]
    fn a_missing_weight_falls_to_the_nearest_one() {
        let fonts = Fonts::bundled();
        // 900 is not bundled; 800 is the nearest.
        let font = fonts.web_font("Inter", 900, false).unwrap();
        assert_eq!(font.weight, 800);
    }

    #[test]
    fn a_missing_italic_falls_back_within_the_family() {
        let fonts = Fonts::bundled();
        // Only 400 and 700 italics are bundled, so 300 italic lands on 400 italic.
        let font = fonts.web_font("Inter", 300, true).unwrap();
        assert!(font.italic);
        assert_eq!(font.weight, 400);
    }

    #[test]
    fn a_system_font_is_never_offered_for_embedding() {
        // Copying a font the user installed into a website they publish is a licensing
        // decision the tool has no business making for them.
        let fonts = Fonts::bundled().with_system_fonts();
        assert!(fonts.web_font("DejaVu Sans", 400, false).is_none());
    }

    #[test]
    fn every_embeddable_face_of_a_family_can_be_listed() {
        let fonts = Fonts::bundled();
        let faces = fonts.web_fonts_for("inter");
        assert_eq!(faces.len(), BUNDLED.len());
        assert!(faces.iter().any(|f| f.italic));
    }
}
