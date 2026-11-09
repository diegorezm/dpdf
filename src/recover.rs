//! Recovers text a PDF draws but doesn't spell out.
//!
//! Some PDFs, most often Word documents with equations, map the glyphs of
//! a font to spaces or to nothing, so PDFium extracts blanks where a
//! formula is drawn. When the font is a Type0 font with Identity
//! encoding, the codes in the content stream are glyph IDs; this module
//! reads them with `lopdf` and turns each back into a character through
//! the font itself: the embedded copy first, then the same font installed
//! on the system (Word keeps the original glyph IDs in its subsets).
//!
//! Only glyphs PDFium has no text for are used; everything else keeps
//! PDFium's text.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::pdf::NormRect;

/// One glyph found in the content stream, with the character its font
/// says it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Glyph {
    pub c: char,
    pub rect: NormRect,
    /// Baseline, as a fraction of the page height from the top.
    pub baseline: f32,
    pub size: f32,
}

/// A document opened for recovery, kept while it is the open one.
pub struct Recovery {
    doc: Document,
    pages: Vec<ObjectId>,
    fonts: HashMap<ObjectId, Option<FontInfo>>,
}

impl Recovery {
    pub fn load(path: &Path, password: Option<&str>) -> Option<Self> {
        let doc = match password {
            Some(password) => Document::load_with_password(path, password).ok()?,
            None => Document::load(path).ok()?,
        };
        let pages = doc.get_pages().into_values().collect();
        Some(Self {
            doc,
            pages,
            fonts: HashMap::new(),
        })
    }

    /// The glyphs of `page` drawn in fonts this module can read, in
    /// content order. Empty for pages it can't handle (rotated, unusual
    /// fonts).
    pub fn glyphs(&mut self, page: usize) -> Vec<Glyph> {
        let Some(&page_id) = self.pages.get(page) else {
            return Vec::new();
        };
        let Some(page_box) = page_box(&self.doc, page_id) else {
            return Vec::new();
        };
        let Ok(fonts) = self.doc.get_page_fonts(page_id) else {
            return Vec::new();
        };
        let mut by_name: HashMap<Vec<u8>, ObjectId> = HashMap::new();
        for (name, dict) in &fonts {
            // Only fonts that are their own objects can be cached.
            if let Some(id) = font_id(&self.doc, page_id, name) {
                if !self.fonts.contains_key(&id) {
                    self.fonts.insert(id, FontInfo::read(&self.doc, dict));
                }
                by_name.insert(name.clone(), id);
            }
        }
        if !by_name
            .values()
            .any(|id| self.fonts.get(id).is_some_and(Option::is_some))
        {
            return Vec::new();
        }
        let content = self.doc.get_page_content(page_id);
        let Ok(content) = Content::decode(&content) else {
            return Vec::new();
        };
        let mut glyphs = Vec::new();
        let mut state = TextState::default();
        for op in &content.operations {
            state.apply(&op.operator, &op.operands, |state, operands| {
                let Some(info) = by_name
                    .get(&state.font)
                    .and_then(|id| self.fonts.get(id))
                    .and_then(Option::as_ref)
                else {
                    state.skip(operands);
                    return;
                };
                state.show(info, operands, &page_box, &mut glyphs);
            });
        }
        glyphs
    }
}

/// The object ID of a font the page names `name`, looked up through the
/// page's (possibly inherited) resources.
fn font_id(doc: &Document, page_id: ObjectId, name: &[u8]) -> Option<ObjectId> {
    let (direct, inherited) = doc.get_page_resources(page_id).ok()?;
    let mut resources: Vec<&Dictionary> = direct.into_iter().collect();
    resources.extend(
        inherited
            .iter()
            .filter_map(|id| doc.get_object(*id).ok()?.as_dict().ok()),
    );
    resources.iter().find_map(|res| {
        let fonts = res.get(b"Font").ok()?;
        let fonts = doc.dereference(fonts).ok()?.1.as_dict().ok()?;
        fonts.get(name).ok()?.as_reference().ok()
    })
}

/// The visible page box (x0, y0, x1, y1) in user space; `None` for rotated
/// pages, which would need the rotation applied.
fn page_box(doc: &Document, page_id: ObjectId) -> Option<[f32; 4]> {
    let rotate = inherited(doc, page_id, b"Rotate")
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(0);
    if rotate.rem_euclid(360) != 0 {
        return None;
    }
    let rect =
        inherited(doc, page_id, b"CropBox").or_else(|| inherited(doc, page_id, b"MediaBox"))?;
    let rect = rect.as_array().ok()?;
    let mut v = [0.0; 4];
    for (slot, n) in v.iter_mut().zip(rect) {
        *slot = doc.dereference(n).ok()?.1.as_float().ok()?;
    }
    let [x0, y0, x1, y1] = v;
    Some([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
}

/// A page attribute, looked up through the page tree's parents.
fn inherited<'a>(doc: &'a Document, page_id: ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut node = doc.get_object(page_id).ok()?.as_dict().ok()?;
    // Bounded, in case the tree is cyclic.
    for _ in 0..32 {
        if let Ok(value) = node.get(key) {
            return doc.dereference(value).ok().map(|(_, o)| o);
        }
        let parent = node.get(b"Parent").ok()?.as_reference().ok()?;
        node = doc.get_object(parent).ok()?.as_dict().ok()?;
    }
    None
}

// ----- Fonts ---------------------------------------------------------------

/// What's needed to place and name the glyphs of a Type0 Identity-H font.
struct FontInfo {
    widths: HashMap<u16, f32>,
    default_width: f32,
    ascent: f32,
    descent: f32,
    chars: GlyphChars,
}

impl FontInfo {
    fn read(doc: &Document, font: &Dictionary) -> Option<Self> {
        if entry(doc, font, b"Subtype")?.as_name().ok()? != b"Type0"
            || entry(doc, font, b"Encoding")?.as_name().ok()? != b"Identity-H"
        {
            return None;
        }
        let cid = entry(doc, font, b"DescendantFonts")?
            .as_array()
            .ok()?
            .first()?;
        let cid = doc.dereference(cid).ok()?.1.as_dict().ok()?;
        // Glyph IDs equal CIDs only with an identity map (the default for
        // TrueType-based CID fonts).
        if let Some(map) = entry(doc, cid, b"CIDToGIDMap") {
            if map.as_name().ok()? != b"Identity" {
                return None;
            }
        }
        let descriptor = entry(doc, cid, b"FontDescriptor")?.as_dict().ok()?;
        let number = |dict: &Dictionary, key: &[u8], default: f32| {
            entry(doc, dict, key)
                .and_then(|o| o.as_float().ok())
                .unwrap_or(default)
        };
        let embedded = [b"FontFile2".as_slice(), b"FontFile3"]
            .iter()
            .find_map(|key| {
                entry(doc, descriptor, key)?
                    .as_stream()
                    .ok()?
                    .decompressed_content()
                    .ok()
            });
        let base = entry(doc, font, b"BaseFont")?.as_name().ok()?;
        let family = String::from_utf8_lossy(base);
        // Subsets are named like "ABCDEE+Cambria Math".
        let family = family.split_once('+').map_or(&*family, |(_, name)| name);
        let chars = GlyphChars::for_font(embedded.as_deref(), family)?;
        Some(Self {
            widths: widths(doc, entry(doc, cid, b"W")),
            default_width: number(cid, b"DW", 1000.0),
            ascent: number(descriptor, b"Ascent", 800.0),
            descent: number(descriptor, b"Descent", -200.0),
            chars,
        })
    }

    fn width(&self, gid: u16) -> f32 {
        self.widths.get(&gid).copied().unwrap_or(self.default_width)
    }
}

/// A dictionary entry, following a reference.
fn entry<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    doc.dereference(dict.get(key).ok()?).ok().map(|(_, o)| o)
}

/// Glyph widths from a CID font's `W` array: `c [w1 w2 …]` or
/// `c_first c_last w`.
fn widths(doc: &Document, w: Option<&Object>) -> HashMap<u16, f32> {
    let mut map = HashMap::new();
    let Some(items) = w.and_then(|w| w.as_array().ok()) else {
        return map;
    };
    let value = |o: &Object| doc.dereference(o).ok().and_then(|(_, o)| o.as_float().ok());
    let mut i = 0;
    while i < items.len() {
        let Some(first) = value(&items[i]).map(|v| v as u32) else {
            break;
        };
        match items.get(i + 1).map(|o| doc.dereference(o).map(|(_, o)| o)) {
            Some(Ok(Object::Array(list))) => {
                for (k, w) in list.iter().enumerate() {
                    if let (Ok(gid), Some(w)) = (u16::try_from(first as usize + k), value(w)) {
                        map.insert(gid, w);
                    }
                }
                i += 2;
            }
            Some(Ok(_)) => {
                let (Some(last), Some(w)) = (
                    items.get(i + 1).and_then(value),
                    items.get(i + 2).and_then(value),
                ) else {
                    break;
                };
                // Bounded so a hostile range can't allocate forever.
                for gid in first..=(last as u32).min(first + 0xFFFF) {
                    if let Ok(gid) = u16::try_from(gid) {
                        map.insert(gid, w);
                    }
                }
                i += 3;
            }
            _ => break,
        }
    }
    map
}

/// Characters for glyph IDs, from the embedded font and, when its glyph
/// IDs line up, the installed font of the same name.
struct GlyphChars {
    embedded: HashMap<u16, char>,
    system: Option<std::sync::Arc<HashMap<u16, char>>>,
}

impl GlyphChars {
    fn for_font(embedded: Option<&[u8]>, family: &str) -> Option<Self> {
        let face = embedded.and_then(|data| ttf_parser::Face::parse(data, 0).ok());
        let embedded_map = face.as_ref().map(glyph_chars).unwrap_or_default();
        let system = system_font(family).filter(|(glyphs, _)| {
            // Without the same glyph count, IDs may have been renumbered.
            face.as_ref()
                .is_none_or(|f| f.number_of_glyphs() == *glyphs)
        });
        if embedded_map.is_empty() && system.is_none() {
            return None;
        }
        Some(Self {
            embedded: embedded_map,
            system: system.map(|(_, map)| map),
        })
    }

    fn get(&self, gid: u16) -> Option<char> {
        self.embedded
            .get(&gid)
            .or_else(|| self.system.as_ref()?.get(&gid))
            .copied()
    }
}

/// Every glyph a font can name: through its Unicode cmap, through glyph
/// names, and, for variants such as script-size digits, through the glyph
/// they substitute (GSUB single and alternate substitutions).
fn glyph_chars(face: &ttf_parser::Face) -> HashMap<u16, char> {
    let mut map: HashMap<u16, char> = HashMap::new();
    if let Some(cmap) = face.tables().cmap {
        for sub in cmap.subtables {
            if !sub.is_unicode() {
                continue;
            }
            sub.codepoints(|cp| {
                if let (Some(gid), Some(c)) = (sub.glyph_index(cp), char::from_u32(cp)) {
                    // Prefer the lowest code point (plain over styled forms).
                    let slot = map.entry(gid.0).or_insert(c);
                    if c < *slot {
                        *slot = c;
                    }
                }
            });
        }
    }
    for gid in 0..face.number_of_glyphs() {
        if map.contains_key(&gid) {
            continue;
        }
        if let Some(c) = face
            .glyph_name(ttf_parser::GlyphId(gid))
            .and_then(char_for_name)
        {
            map.insert(gid, c);
        }
    }
    let Some(gsub) = face.tables().gsub else {
        return map;
    };
    let mut variants: Vec<(u16, u16)> = Vec::new();
    for lookup in gsub.lookups {
        for i in 0..lookup.subtables.len() {
            use ttf_parser::gsub::{SingleSubstitution, SubstitutionSubtable};
            match lookup.subtables.get::<SubstitutionSubtable>(i) {
                Some(SubstitutionSubtable::Single(SingleSubstitution::Format1 {
                    coverage,
                    delta,
                })) => {
                    for_coverage(&coverage, |gid, _| {
                        variants.push((gid, (gid as i32 + delta as i32) as u16));
                    });
                }
                Some(SubstitutionSubtable::Single(SingleSubstitution::Format2 {
                    coverage,
                    substitutes,
                })) => {
                    for_coverage(&coverage, |gid, index| {
                        if let Some(out) = substitutes.get(index) {
                            variants.push((gid, out.0));
                        }
                    });
                }
                Some(SubstitutionSubtable::Alternate(alt)) => {
                    for_coverage(&alt.coverage, |gid, index| {
                        if let Some(set) = alt.alternate_sets.get(index) {
                            variants.extend(set.alternates.into_iter().map(|out| (gid, out.0)));
                        }
                    });
                }
                _ => {}
            }
        }
    }
    // Variants of variants resolve over a few passes.
    for _ in 0..3 {
        for &(base, variant) in &variants {
            if !map.contains_key(&variant) {
                if let Some(&c) = map.get(&base) {
                    map.insert(variant, c);
                }
            }
        }
    }
    map
}

/// Calls `f(glyph, coverage index)` for each glyph a coverage table lists.
fn for_coverage(coverage: &ttf_parser::opentype_layout::Coverage, mut f: impl FnMut(u16, u16)) {
    use ttf_parser::opentype_layout::Coverage;
    match coverage {
        Coverage::Format1 { glyphs } => {
            for (i, gid) in glyphs.into_iter().enumerate() {
                f(gid.0, i as u16);
            }
        }
        Coverage::Format2 { records } => {
            for record in records.into_iter() {
                for gid in record.start.0..=record.end.0 {
                    f(gid, record.value.wrapping_add(gid - record.start.0));
                }
            }
        }
    }
}

/// The character an Adobe glyph name stands for: `uniXXXX`, `uXXXXX`, a
/// single character, or a few common names. Suffixes like `.ssty` are
/// ignored.
fn char_for_name(name: &str) -> Option<char> {
    let base = name.split('.').next().unwrap_or(name);
    let hex = base
        .strip_prefix("uni")
        .filter(|h| h.len() == 4)
        .or_else(|| {
            base.strip_prefix('u')
                .filter(|h| (4..=6).contains(&h.len()))
        });
    if let Some(c) = hex
        .and_then(|h| u32::from_str_radix(h, 16).ok())
        .and_then(char::from_u32)
    {
        return Some(c);
    }
    let mut chars = base.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(c);
    }
    const NAMES: [(&str, char); 18] = [
        ("zero", '0'),
        ("one", '1'),
        ("two", '2'),
        ("three", '3'),
        ("four", '4'),
        ("five", '5'),
        ("six", '6'),
        ("seven", '7'),
        ("eight", '8'),
        ("nine", '9'),
        ("plus", '+'),
        ("minus", '−'),
        ("equal", '='),
        ("parenleft", '('),
        ("parenright", ')'),
        ("comma", ','),
        ("period", '.'),
        ("slash", '/'),
    ];
    NAMES.iter().find(|(n, _)| *n == base).map(|&(_, c)| c)
}

// ----- System fonts --------------------------------------------------------

type SystemFont = Option<(u16, std::sync::Arc<HashMap<u16, char>>)>;

/// The glyph count and glyph characters of an installed font named
/// `family`, cached for the session. Only fonts the PDF names are read.
fn system_font(family: &str) -> SystemFont {
    static CACHE: OnceLock<Mutex<HashMap<String, SystemFont>>> = OnceLock::new();
    let key = name_key(family);
    let cache = CACHE.get_or_init(Default::default);
    if let Some(found) = cache.lock().ok()?.get(&key) {
        return found.clone();
    }
    let found = find_font_file(&key).and_then(|(path, index)| {
        let data = std::fs::read(path).ok()?;
        let face = ttf_parser::Face::parse(&data, index).ok()?;
        Some((
            face.number_of_glyphs(),
            std::sync::Arc::new(glyph_chars(&face)),
        ))
    });
    if let Ok(mut cache) = cache.lock() {
        cache.insert(key, found.clone());
    }
    found
}

/// Font names compared without case, spaces, or hyphens, so the PDF's
/// "CambriaMath" finds "Cambria Math".
fn name_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(windows) = std::env::var_os("WINDIR") {
        dirs.push(PathBuf::from(windows).join("Fonts"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Microsoft\\Windows\\Fonts"));
    }
    dirs
}

/// The file and collection index of the installed font whose family,
/// full, or PostScript name matches `key`. Reads only each file's name
/// table.
fn find_font_file(key: &str) -> Option<(PathBuf, u32)> {
    for dir in font_dirs() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_font = path.extension().is_some_and(|e| {
                ["ttf", "otf", "ttc"]
                    .iter()
                    .any(|x| e.eq_ignore_ascii_case(x))
            });
            if !is_font {
                continue;
            }
            if let Some(index) = font_index_named(&path, key) {
                return Some((path, index));
            }
        }
    }
    None
}

fn font_index_named(path: &Path, key: &str) -> Option<u32> {
    let mut file = File::open(path).ok()?;
    let mut head = [0u8; 12];
    file.read_exact(&mut head).ok()?;
    let offsets: Vec<u32> = if &head[..4] == b"ttcf" {
        let count = u32::from_be_bytes(head[8..12].try_into().ok()?).min(64);
        let mut table = vec![0u8; count as usize * 4];
        file.read_exact(&mut table).ok()?;
        table
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_be_bytes(*b))
            .collect()
    } else {
        vec![0]
    };
    offsets.iter().enumerate().find_map(|(index, &offset)| {
        let names = name_table(&mut file, offset)?;
        let table = ttf_parser::name::Table::parse(&names)?;
        let matches = table.names.into_iter().any(|name| {
            matches!(name.name_id, 1 | 4 | 6)
                && name.to_string().is_some_and(|n| name_key(&n) == key)
        });
        matches.then_some(index as u32)
    })
}

/// The `name` table of the font starting at `offset` in `file`.
fn name_table(file: &mut File, offset: u32) -> Option<Vec<u8>> {
    file.seek(SeekFrom::Start(u64::from(offset) + 4)).ok()?;
    let mut count = [0u8; 2];
    file.read_exact(&mut count).ok()?;
    let count = u16::from_be_bytes(count).min(128) as usize;
    file.seek(SeekFrom::Start(u64::from(offset) + 12)).ok()?;
    let mut records = vec![0u8; count * 16];
    file.read_exact(&mut records).ok()?;
    let record = records
        .as_chunks::<16>()
        .0
        .iter()
        .find(|r| &r[..4] == b"name")?;
    let start = u32::from_be_bytes(record[8..12].try_into().ok()?);
    let len = u32::from_be_bytes(record[12..16].try_into().ok()?).min(1 << 20);
    file.seek(SeekFrom::Start(u64::from(start))).ok()?;
    let mut data = vec![0u8; len as usize];
    file.read_exact(&mut data).ok()?;
    Some(data)
}

// ----- Content stream ------------------------------------------------------

/// A 2D affine matrix [a b c d e f], applied to row vectors as in PDF.
type Matrix = [f32; 6];

const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn multiply(m: &Matrix, n: &Matrix) -> Matrix {
    [
        m[0] * n[0] + m[1] * n[2],
        m[0] * n[1] + m[1] * n[3],
        m[2] * n[0] + m[3] * n[2],
        m[2] * n[1] + m[3] * n[3],
        m[4] * n[0] + m[5] * n[2] + n[4],
        m[4] * n[1] + m[5] * n[3] + n[5],
    ]
}

fn apply(m: &Matrix, (x, y): (f32, f32)) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// The graphics and text state that positions glyphs.
struct TextState {
    ctm: Matrix,
    stack: Vec<Matrix>,
    tm: Matrix,
    tlm: Matrix,
    font: Vec<u8>,
    size: f32,
    char_spacing: f32,
    word_spacing: f32,
    scale: f32,
    leading: f32,
    rise: f32,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            ctm: IDENTITY,
            stack: Vec::new(),
            tm: IDENTITY,
            tlm: IDENTITY,
            font: Vec::new(),
            size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            scale: 1.0,
            leading: 0.0,
            rise: 0.0,
        }
    }
}

fn numbers<const N: usize>(operands: &[Object]) -> Option<[f32; N]> {
    let mut out = [0.0; N];
    if operands.len() < N {
        return None;
    }
    for (slot, o) in out.iter_mut().zip(operands) {
        *slot = o.as_float().ok()?;
    }
    Some(out)
}

impl TextState {
    /// Updates the state for one operator; `show` is called for operators
    /// that draw text, with the strings and kerning to draw.
    fn apply(&mut self, op: &str, operands: &[Object], mut show: impl FnMut(&mut Self, &[Object])) {
        match op {
            "q" => {
                // Bounded: a hostile stream can't grow it without limit.
                if self.stack.len() < 256 {
                    self.stack.push(self.ctm);
                }
            }
            "Q" => self.ctm = self.stack.pop().unwrap_or(IDENTITY),
            "cm" => {
                if let Some(m) = numbers::<6>(operands) {
                    self.ctm = multiply(&m, &self.ctm);
                }
            }
            "BT" => {
                self.tm = IDENTITY;
                self.tlm = IDENTITY;
            }
            "Tf" => {
                if let (Some(Ok(name)), Some(Ok(size))) = (
                    operands.first().map(Object::as_name),
                    operands.get(1).map(Object::as_float),
                ) {
                    self.font = name.to_vec();
                    self.size = size;
                }
            }
            "Td" => {
                if let Some([x, y]) = numbers::<2>(operands) {
                    self.next_line(x, y);
                }
            }
            "TD" => {
                if let Some([x, y]) = numbers::<2>(operands) {
                    self.leading = -y;
                    self.next_line(x, y);
                }
            }
            "Tm" => {
                if let Some(m) = numbers::<6>(operands) {
                    self.tm = m;
                    self.tlm = m;
                }
            }
            "T*" => self.next_line(0.0, -self.leading),
            "TL" => self.leading = numbers::<1>(operands).map_or(self.leading, |[v]| v),
            "Tc" => self.char_spacing = numbers::<1>(operands).map_or(self.char_spacing, |[v]| v),
            "Tw" => self.word_spacing = numbers::<1>(operands).map_or(self.word_spacing, |[v]| v),
            "Tz" => self.scale = numbers::<1>(operands).map_or(self.scale, |[v]| v / 100.0),
            "Ts" => self.rise = numbers::<1>(operands).map_or(self.rise, |[v]| v),
            "Tj" | "TJ" => show(self, operands),
            "'" => {
                self.next_line(0.0, -self.leading);
                show(self, operands);
            }
            "\"" => {
                if let Some([aw, ac]) = numbers::<2>(operands) {
                    self.word_spacing = aw;
                    self.char_spacing = ac;
                }
                self.next_line(0.0, -self.leading);
                show(self, operands.get(2..).unwrap_or_default());
            }
            _ => {}
        }
    }

    fn next_line(&mut self, x: f32, y: f32) {
        self.tlm = multiply(&[1.0, 0.0, 0.0, 1.0, x, y], &self.tlm);
        self.tm = self.tlm;
    }

    fn advance(&mut self, tx: f32) {
        self.tm = multiply(&[1.0, 0.0, 0.0, 1.0, tx, 0.0], &self.tm);
    }

    /// Text in a font this module can't read: positions after it can't be
    /// known exactly, but the next `Td`/`Tm` resets them, which is how
    /// Word and most generators write each run.
    fn skip(&mut self, _operands: &[Object]) {}

    /// Places the glyphs of a `Tj`/`TJ` operand list.
    fn show(
        &mut self,
        font: &FontInfo,
        operands: &[Object],
        page: &[f32; 4],
        out: &mut Vec<Glyph>,
    ) {
        let items: Vec<&Object> = match operands.first() {
            Some(Object::Array(items)) => items.iter().collect(),
            _ => operands.iter().collect(),
        };
        for item in items {
            match item {
                Object::String(bytes, _) => {
                    for code in bytes.as_chunks::<2>().0 {
                        let gid = u16::from_be_bytes(*code);
                        let width = font.width(gid) / 1000.0;
                        if let Some(c) = font.chars.get(gid) {
                            if let Some(glyph) = self.place(font, c, width, page) {
                                out.push(glyph);
                            }
                        }
                        self.advance((width * self.size + self.char_spacing) * self.scale);
                    }
                }
                other => {
                    if let Ok(kern) = other.as_float() {
                        self.advance(-kern / 1000.0 * self.size * self.scale);
                    }
                }
            }
        }
    }

    fn place(&self, font: &FontInfo, c: char, width: f32, page: &[f32; 4]) -> Option<Glyph> {
        let trm = multiply(
            &multiply(
                &[self.size * self.scale, 0.0, 0.0, self.size, 0.0, self.rise],
                &self.tm,
            ),
            &self.ctm,
        );
        let corners = [
            apply(&trm, (0.0, font.descent / 1000.0)),
            apply(&trm, (width, font.ascent / 1000.0)),
        ];
        let (x0, x1) = (
            corners[0].0.min(corners[1].0),
            corners[0].0.max(corners[1].0),
        );
        let (y0, y1) = (
            corners[0].1.min(corners[1].1),
            corners[0].1.max(corners[1].1),
        );
        let [px0, py0, px1, py1] = *page;
        let (w, h) = (px1 - px0, py1 - py0);
        if !(w > 0.0 && h > 0.0) {
            return None;
        }
        let baseline = apply(&trm, (0.0, 0.0)).1;
        // Effective size on the page, which `Tf` alone may not give.
        let size = (trm[2] * trm[2] + trm[3] * trm[3]).sqrt();
        Some(Glyph {
            c,
            rect: NormRect {
                x: (x0 - px0) / w,
                y: (py1 - y1) / h,
                width: (x1 - x0) / w,
                height: (y1 - y0) / h,
            },
            baseline: (py1 - baseline) / h,
            size,
        })
    }
}

// ----- Text ----------------------------------------------------------------

/// Plain letters and digits for the styled ones in Mathematical
/// Alphanumeric Symbols (𝑥 → x), so copied formulas read normally.
pub fn plain(c: char) -> char {
    let cp = c as u32;
    if c == 'ℎ' {
        return 'h';
    }
    match cp {
        // Thirteen styles of A–Z a–z.
        0x1D400..=0x1D6A3 => {
            let i = (cp - 0x1D400) % 52;
            let base = if i < 26 {
                'A' as u32 + i
            } else {
                'a' as u32 + i - 26
            };
            char::from_u32(base).unwrap_or(c)
        }
        // Five styles of 0–9.
        0x1D7CE..=0x1D7FF => char::from_u32('0' as u32 + (cp - 0x1D7CE) % 10).unwrap_or(c),
        _ => c,
    }
}

/// `c` written as a superscript or subscript, or as is when Unicode has
/// no such form.
pub fn script(c: char, sup: bool) -> String {
    const SUP: [(char, char); 17] = [
        ('0', '⁰'),
        ('1', '¹'),
        ('2', '²'),
        ('3', '³'),
        ('4', '⁴'),
        ('5', '⁵'),
        ('6', '⁶'),
        ('7', '⁷'),
        ('8', '⁸'),
        ('9', '⁹'),
        ('+', '⁺'),
        ('−', '⁻'),
        ('-', '⁻'),
        ('=', '⁼'),
        ('(', '⁽'),
        (')', '⁾'),
        ('n', 'ⁿ'),
    ];
    const SUB: [(char, char); 16] = [
        ('0', '₀'),
        ('1', '₁'),
        ('2', '₂'),
        ('3', '₃'),
        ('4', '₄'),
        ('5', '₅'),
        ('6', '₆'),
        ('7', '₇'),
        ('8', '₈'),
        ('9', '₉'),
        ('+', '₊'),
        ('−', '₋'),
        ('-', '₋'),
        ('=', '₌'),
        ('(', '₍'),
        (')', '₎'),
    ];
    let table: &[(char, char)] = if sup { &SUP } else { &SUB };
    match table.iter().find(|(from, _)| *from == c) {
        Some(&(_, to)) => to.to_string(),
        None => c.to_string(),
    }
}

/// The text of a run of glyphs: plain letters, with a short group of
/// smaller glyphs right after a full-size one written as a superscript
/// or subscript, by whether it sits above or below that glyph's
/// baseline. Longer groups of small glyphs (the numerator of a fraction)
/// stay plain: two-dimensional layout has no linear form.
pub fn run_text(run: &[Glyph]) -> Vec<String> {
    let base = run.iter().map(|g| g.size).fold(0.0, f32::max);
    let full = |g: &Glyph| g.size >= base * 0.85;
    let mut out = Vec::with_capacity(run.len());
    let mut i = 0;
    while i < run.len() {
        if i == 0 || full(&run[i]) {
            out.push(plain(run[i].c).to_string());
            i += 1;
            continue;
        }
        let end = run[i..].iter().position(full).map_or(run.len(), |n| i + n);
        let baseline = run[i - 1].baseline;
        let scripted = end - i <= 3;
        for g in &run[i..end] {
            let c = plain(g.c);
            out.push(match () {
                // Page fractions grow downward: smaller is higher.
                _ if scripted && g.baseline < baseline => script(c, true),
                _ if scripted && g.baseline > baseline => script(c, false),
                _ => c.to_string(),
            });
        }
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(c: char, size: f32, baseline: f32) -> Glyph {
        Glyph {
            c,
            rect: NormRect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            },
            baseline,
            size,
        }
    }

    #[test]
    fn styled_math_letters_become_plain() {
        assert_eq!(plain('𝑥'), 'x');
        assert_eq!(plain('𝐀'), 'A');
        assert_eq!(plain('𝟐'), '2');
        assert_eq!(plain('ℎ'), 'h');
        assert_eq!(plain('−'), '−');
    }

    #[test]
    fn small_raised_glyphs_are_superscripts() {
        let run = [
            glyph('𝑥', 11.0, 0.100),
            glyph('2', 8.0, 0.095),
            glyph('−', 11.0, 0.100),
            glyph('𝑦', 11.0, 0.100),
            glyph('1', 8.0, 0.104),
            glyph('𝑘', 8.0, 0.090),
        ];
        let text: String = run_text(&run).concat();
        assert_eq!(text, "x²−y₁k");
    }

    #[test]
    fn glyph_names_resolve() {
        assert_eq!(char_for_name("two.ssty"), Some('2'));
        assert_eq!(char_for_name("uni2212"), Some('−'));
        assert_eq!(char_for_name("u1D465"), Some('𝑥'));
        assert_eq!(char_for_name("x"), Some('x'));
        assert_eq!(char_for_name("glyph02870"), None);
    }

    #[test]
    fn text_matrix_moves_glyphs() {
        let m = multiply(
            &[1.0, 0.0, 0.0, 1.0, 10.0, 0.0],
            &[2.0, 0.0, 0.0, 2.0, 5.0, 5.0],
        );
        assert_eq!(apply(&m, (1.0, 1.0)), (27.0, 7.0));
        assert_eq!(name_key("ABCDEE Cambria-Math"), "abcdeecambriamath");
    }
}
