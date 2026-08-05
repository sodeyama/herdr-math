//! Construction of Kitty graphics protocol escape sequences.
//!
//! These functions build the byte sequences that transmit, place, or delete
//! images in a Kitty-graphics-capable terminal, plus the `placeholder_grid`
//! used to glue a virtual placement to real scrollback cells. No bytes are
//! written here; callers forward the returned sequences to the terminal.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use miniz_oxide::deflate::compress_to_vec_zlib;

/// Maximum size of each base64 chunk inside one inline transmit.
pub const KITTY_CHUNK_SIZE: usize = 4096;

/// Returns true when the process runs inside tmux.
pub fn inside_tmux() -> bool {
    std::env::var_os("TMUX").is_some()
}

/// Wraps one terminal control string in tmux's passthrough envelope.
///
/// tmux requires every `ESC` in the payload to be doubled. The outer opener
/// and terminator remain single escapes.
pub fn dcs_wrap(bytes: &[u8]) -> Vec<u8> {
    let escapes = bytes.iter().filter(|&&byte| byte == 0x1b).count();
    let mut out = Vec::with_capacity(bytes.len() + escapes + 8);
    out.extend_from_slice(b"\x1bPtmux;");
    for &byte in bytes {
        if byte == 0x1b {
            out.push(0x1b);
        }
        out.push(byte);
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

/// How an image is anchored to the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// The image sits at the cursor and does not move it (`C=1`).
    Cursor,
    /// A virtual placement over real cells, so the image scrolls with the
    /// terminal scrollback.
    Cells { cols: u32, rows: u32 },
}

impl Placement {
    fn keys(self) -> String {
        match self {
            // C=1 keeps the cursor put after display, so a full-window image
            // cannot push the cursor past the last row and force a scroll.
            Placement::Cursor => "p=1,C=1".to_string(),
            Placement::Cells { cols, rows } => format!("U=1,c={cols},r={rows}"),
        }
    }
}

/// Secondary medium through which an image name or payload is carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Medium {
    /// Transmit the compressed pixels inline as base64 (`t=d`).
    Inline,
    /// Reference data already stored under a name (`t=s` shared memory) or
    /// file (`t=f` media) rather than re-sending the pixels.
    Named,
}

impl Medium {
    fn key(self) -> char {
        match self {
            Medium::Inline => 'd',
            Medium::Named => 's',
        }
    }
}

/// Builds an `a=q` probe asking whether a named medium of the given size can be
/// used. The reply is delivered through the terminal's normal response channel.
pub fn kitty_query_medium(
    image_id: u32,
    name: &str,
    medium: Medium,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let payload = BASE64.encode(name);
    let t = medium.key();
    format!("\x1b_Gi={image_id},a=q,t={t},f=32,s={width},v={height};{payload}\x1b\\").into_bytes()
}

/// Builds a transmit that names an already-stored medium instead of carrying
/// pixels, used with file (`t=f`) or shared-memory (`t=s`) media.
pub fn kitty_transmit_named(
    image_id: u32,
    width: u32,
    height: u32,
    name: &str,
    medium: Medium,
    placement: Placement,
) -> Vec<u8> {
    let payload = BASE64.encode(name);
    let keys = placement.keys();
    let t = medium.key();
    format!("\x1b_Ga=T,f=32,s={width},v={height},t={t},i={image_id},{keys},q=2;{payload}\x1b\\")
        .into_bytes()
}

/// Builds a cursor-anchored transmit of an RGBA canvas.
pub fn kitty_transmit(image_id: u32, width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    kitty_transmit_placed(image_id, width, height, rgba, Placement::Cursor)
}

/// Builds a chunked transmit of an RGBA canvas anchored to the given placement.
pub fn kitty_transmit_placed(
    image_id: u32,
    width: u32,
    height: u32,
    rgba: &[u8],
    placement: Placement,
) -> Vec<u8> {
    kitty_transmit_placed_commands(image_id, width, height, rgba, placement)
        .into_iter()
        .flatten()
        .collect()
}

/// Builds one independently framed Kitty APC command per upload chunk.
///
/// Keeping command boundaries lets tmux transports wrap each APC separately
/// without swallowing pane-local cursor or placeholder output.
pub fn kitty_transmit_placed_commands(
    image_id: u32,
    width: u32,
    height: u32,
    rgba: &[u8],
    placement: Placement,
) -> Vec<Vec<u8>> {
    assert_eq!(
        rgba.len(),
        (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4)
    );
    let compressed = compress_to_vec_zlib(rgba, 1);
    let payload = BASE64.encode(&compressed);
    let chunks: Vec<&[u8]> = payload.as_bytes().chunks(KITTY_CHUNK_SIZE).collect();
    let last = chunks.len() - 1;

    let mut out = Vec::with_capacity(chunks.len());
    let mut seq = Vec::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i != last);
        seq.clear();
        seq.extend_from_slice(b"\x1b_G");
        if i == 0 {
            let keys = placement.keys();
            seq.extend_from_slice(
                format!("a=T,f=32,o=z,s={width},v={height},t=d,i={image_id},{keys},q=2,m={more}")
                    .as_bytes(),
            );
        } else {
            seq.extend_from_slice(format!("m={more}").as_bytes());
        }
        seq.push(b';');
        seq.extend_from_slice(chunk);
        seq.extend_from_slice(b"\x1b\\");
        out.push(seq.clone());
    }
    out
}

/// Builds the sequence that deletes this toolkit's images from the terminal.
pub fn kitty_delete(_image_id: u32) -> Vec<u8> {
    b"\x1b_Ga=d,d=A,q=2\x1b\\".to_vec()
}

/// Builds the sequence that deletes one image by id, leaving every other image
/// intact. Used to replace or remove a single placed block.
pub fn kitty_delete_id(image_id: u32) -> Vec<u8> {
    format!("\x1b_Ga=d,d=I,i={image_id},q=2\x1b\\").into_bytes()
}

/// The character used on each placeholder cell. Persisted text must never
/// contain it; it is a private-use character reserved for this protocol.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// Diacritics that encode row/column indices on placeholder cells, from
/// https://sw.kovidgoyal.net/kitty/_downloads/f0a0de9ec8d9ff4456206db8e0814937/rowcolumn-diacritics.txt
pub(crate) const ROW_COLUMN_DIACRITICS: [char; 297] = [
    '\u{0305}',
    '\u{030D}',
    '\u{030E}',
    '\u{0310}',
    '\u{0312}',
    '\u{033D}',
    '\u{033E}',
    '\u{033F}',
    '\u{0346}',
    '\u{034A}',
    '\u{034B}',
    '\u{034C}',
    '\u{0350}',
    '\u{0351}',
    '\u{0352}',
    '\u{0357}',
    '\u{035B}',
    '\u{0363}',
    '\u{0364}',
    '\u{0365}',
    '\u{0366}',
    '\u{0367}',
    '\u{0368}',
    '\u{0369}',
    '\u{036A}',
    '\u{036B}',
    '\u{036C}',
    '\u{036D}',
    '\u{036E}',
    '\u{036F}',
    '\u{0483}',
    '\u{0484}',
    '\u{0485}',
    '\u{0486}',
    '\u{0487}',
    '\u{0592}',
    '\u{0593}',
    '\u{0594}',
    '\u{0595}',
    '\u{0597}',
    '\u{0598}',
    '\u{0599}',
    '\u{059C}',
    '\u{059D}',
    '\u{059E}',
    '\u{059F}',
    '\u{05A0}',
    '\u{05A1}',
    '\u{05A8}',
    '\u{05A9}',
    '\u{05AB}',
    '\u{05AC}',
    '\u{05AF}',
    '\u{05C4}',
    '\u{0610}',
    '\u{0611}',
    '\u{0612}',
    '\u{0613}',
    '\u{0614}',
    '\u{0615}',
    '\u{0616}',
    '\u{0617}',
    '\u{0657}',
    '\u{0658}',
    '\u{0659}',
    '\u{065A}',
    '\u{065B}',
    '\u{065D}',
    '\u{065E}',
    '\u{06D6}',
    '\u{06D7}',
    '\u{06D8}',
    '\u{06D9}',
    '\u{06DA}',
    '\u{06DB}',
    '\u{06DC}',
    '\u{06DF}',
    '\u{06E0}',
    '\u{06E1}',
    '\u{06E2}',
    '\u{06E4}',
    '\u{06E7}',
    '\u{06E8}',
    '\u{06EB}',
    '\u{06EC}',
    '\u{0730}',
    '\u{0732}',
    '\u{0733}',
    '\u{0735}',
    '\u{0736}',
    '\u{073A}',
    '\u{073D}',
    '\u{073F}',
    '\u{0740}',
    '\u{0741}',
    '\u{0743}',
    '\u{0745}',
    '\u{0747}',
    '\u{0749}',
    '\u{074A}',
    '\u{07EB}',
    '\u{07EC}',
    '\u{07ED}',
    '\u{07EE}',
    '\u{07EF}',
    '\u{07F0}',
    '\u{07F1}',
    '\u{07F3}',
    '\u{0816}',
    '\u{0817}',
    '\u{0818}',
    '\u{0819}',
    '\u{081B}',
    '\u{081C}',
    '\u{081D}',
    '\u{081E}',
    '\u{081F}',
    '\u{0820}',
    '\u{0821}',
    '\u{0822}',
    '\u{0823}',
    '\u{0825}',
    '\u{0826}',
    '\u{0827}',
    '\u{0829}',
    '\u{082A}',
    '\u{082B}',
    '\u{082C}',
    '\u{082D}',
    '\u{0951}',
    '\u{0953}',
    '\u{0954}',
    '\u{0F82}',
    '\u{0F83}',
    '\u{0F86}',
    '\u{0F87}',
    '\u{135D}',
    '\u{135E}',
    '\u{135F}',
    '\u{17DD}',
    '\u{193A}',
    '\u{1A17}',
    '\u{1A75}',
    '\u{1A76}',
    '\u{1A77}',
    '\u{1A78}',
    '\u{1A79}',
    '\u{1A7A}',
    '\u{1A7B}',
    '\u{1A7C}',
    '\u{1B6B}',
    '\u{1B6D}',
    '\u{1B6E}',
    '\u{1B6F}',
    '\u{1B70}',
    '\u{1B71}',
    '\u{1B72}',
    '\u{1B73}',
    '\u{1CD0}',
    '\u{1CD1}',
    '\u{1CD2}',
    '\u{1CDA}',
    '\u{1CDB}',
    '\u{1CE0}',
    '\u{1DC0}',
    '\u{1DC1}',
    '\u{1DC3}',
    '\u{1DC4}',
    '\u{1DC5}',
    '\u{1DC6}',
    '\u{1DC7}',
    '\u{1DC8}',
    '\u{1DC9}',
    '\u{1DCB}',
    '\u{1DCC}',
    '\u{1DD1}',
    '\u{1DD2}',
    '\u{1DD3}',
    '\u{1DD4}',
    '\u{1DD5}',
    '\u{1DD6}',
    '\u{1DD7}',
    '\u{1DD8}',
    '\u{1DD9}',
    '\u{1DDA}',
    '\u{1DDB}',
    '\u{1DDC}',
    '\u{1DDD}',
    '\u{1DDE}',
    '\u{1DDF}',
    '\u{1DE0}',
    '\u{1DE1}',
    '\u{1DE2}',
    '\u{1DE3}',
    '\u{1DE4}',
    '\u{1DE5}',
    '\u{1DE6}',
    '\u{1DFE}',
    '\u{20D0}',
    '\u{20D1}',
    '\u{20D4}',
    '\u{20D5}',
    '\u{20D6}',
    '\u{20D7}',
    '\u{20DB}',
    '\u{20DC}',
    '\u{20E1}',
    '\u{20E7}',
    '\u{20E9}',
    '\u{20F0}',
    '\u{2CEF}',
    '\u{2CF0}',
    '\u{2CF1}',
    '\u{2DE0}',
    '\u{2DE1}',
    '\u{2DE2}',
    '\u{2DE3}',
    '\u{2DE4}',
    '\u{2DE5}',
    '\u{2DE6}',
    '\u{2DE7}',
    '\u{2DE8}',
    '\u{2DE9}',
    '\u{2DEA}',
    '\u{2DEB}',
    '\u{2DEC}',
    '\u{2DED}',
    '\u{2DEE}',
    '\u{2DEF}',
    '\u{2DF0}',
    '\u{2DF1}',
    '\u{2DF2}',
    '\u{2DF3}',
    '\u{2DF4}',
    '\u{2DF5}',
    '\u{2DF6}',
    '\u{2DF7}',
    '\u{2DF8}',
    '\u{2DF9}',
    '\u{2DFA}',
    '\u{2DFB}',
    '\u{2DFC}',
    '\u{2DFD}',
    '\u{2DFE}',
    '\u{2DFF}',
    '\u{A66F}',
    '\u{A67C}',
    '\u{A67D}',
    '\u{A6F0}',
    '\u{A6F1}',
    '\u{A8E0}',
    '\u{A8E1}',
    '\u{A8E2}',
    '\u{A8E3}',
    '\u{A8E4}',
    '\u{A8E5}',
    '\u{A8E6}',
    '\u{A8E7}',
    '\u{A8E8}',
    '\u{A8E9}',
    '\u{A8EA}',
    '\u{A8EB}',
    '\u{A8EC}',
    '\u{A8ED}',
    '\u{A8EE}',
    '\u{A8EF}',
    '\u{A8F0}',
    '\u{A8F1}',
    '\u{AAB0}',
    '\u{AAB2}',
    '\u{AAB3}',
    '\u{AAB7}',
    '\u{AAB8}',
    '\u{AABE}',
    '\u{AABF}',
    '\u{AAC1}',
    '\u{FE20}',
    '\u{FE21}',
    '\u{FE22}',
    '\u{FE23}',
    '\u{FE24}',
    '\u{FE25}',
    '\u{FE26}',
    '\u{10A0F}',
    '\u{10A38}',
    '\u{1D185}',
    '\u{1D186}',
    '\u{1D187}',
    '\u{1D188}',
    '\u{1D189}',
    '\u{1D1AA}',
    '\u{1D1AB}',
    '\u{1D1AC}',
    '\u{1D1AD}',
    '\u{1D242}',
    '\u{1D243}',
    '\u{1D244}',
];

/// Upper bound on placeholder cells addressable by the diacritic alphabet.
pub const MAX_PLACEHOLDER_CELLS: u32 = ROW_COLUMN_DIACRITICS.len() as u32;

/// The row/column diacritic a placeholder cell at `index` uses (see
/// [`ROW_COLUMN_DIACRITICS`]'s doc comment for the source table). Exposed as
/// an accessor rather than the raw table so cross-crate callers (contract
/// tests asserting a specific row/column was or was not drawn, e.g.
/// `tmath`'s `scroll_region` module tests) can identify a specific
/// placeholder cell's position without depending on the table's internal
/// layout. Returns `None` for an out-of-range index rather than panicking.
pub fn row_column_diacritic(index: u32) -> Option<char> {
    ROW_COLUMN_DIACRITICS.get(index as usize).copied()
}

/// Builds the per-cell grid that keeps a virtual placement glued to real
/// scrollback cells. Each cell holds the placeholder glyph plus two combining
/// diacritics encoding the cell's row and column, and the image id is encoded
/// as the text color so the terminal can look the placement up on scrollback.
pub fn placeholder_grid(image_id: u32, cols: u32, rows: u32) -> Vec<u8> {
    let cols = cols.min(MAX_PLACEHOLDER_CELLS);
    let rows = rows.min(MAX_PLACEHOLDER_CELLS);
    // Ids up to 255 use the 256-indexed foreground form: terminal relays that
    // re-render cells (observed with a session daemon between tmux and the
    // outer terminal) can drop 24-bit foreground colors, which destroys the
    // id association and leaves placeholder cells blank. The indexed form
    // survives those relays and is equally valid per the Kitty spec.
    let mut out = if image_id <= 0xff {
        format!("\x1b[38;5;{image_id}m")
    } else {
        let (r, g, b) = (image_id >> 16 & 0xff, image_id >> 8 & 0xff, image_id & 0xff);
        format!("\x1b[38;2;{r};{g};{b}m")
    };
    for row in 0..rows {
        out.push_str(&format!("\x1b[{};1H", row + 1));
        for col in 0..cols {
            out.push(PLACEHOLDER);
            out.push(ROW_COLUMN_DIACRITICS[row as usize]);
            out.push(ROW_COLUMN_DIACRITICS[col as usize]);
        }
    }
    out.push_str("\x1b[39m");
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dcs_wrap_uses_the_tmux_passthrough_envelope() {
        let wrapped = dcs_wrap(b"\x1b_Ga=T,q=2\x1b\\");
        assert_eq!(wrapped, b"\x1bPtmux;\x1b\x1b_Ga=T,q=2\x1b\x1b\\\x1b\\");
    }

    #[test]
    fn transmit_commands_preserve_apc_chunk_boundaries() {
        let mut seed = 0x12345678u32;
        let pixels: Vec<u8> = (0..64 * 64 * 4)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 24) as u8
            })
            .collect();
        let commands = kitty_transmit_placed_commands(
            1,
            64,
            64,
            &pixels,
            Placement::Cells { cols: 8, rows: 4 },
        );
        assert!(commands.len() > 1);
        assert!(commands
            .iter()
            .all(|command| command.starts_with(b"\x1b_G") && command.ends_with(b"\x1b\\")));
    }

    #[test]
    fn transmit_emits_single_chunk_for_small_images() {
        let out = kitty_transmit(1, 1, 1, &[0xff, 0x00, 0x00, 0xff]);
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("\x1b_Ga=T,f=32,o=z,s=1,v=1,t=d,i=1,p=1,C=1,q=2,m=0;"));
        assert!(text.ends_with("\x1b\\"));

        let payload = text
            .split_once(';')
            .and_then(|(_, rest)| rest.strip_suffix("\x1b\\"))
            .unwrap();
        let decompressed =
            miniz_oxide::inflate::decompress_to_vec_zlib(&BASE64.decode(payload).unwrap()).unwrap();
        assert_eq!(decompressed, [0xff, 0x00, 0x00, 0xff]);
    }

    #[test]
    fn transmit_chunks_large_payloads() {
        let mut seed = 0x12345678u32;
        let pixels: Vec<u8> = (0..64 * 64 * 4)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 24) as u8
            })
            .collect();
        let out = kitty_transmit(1, 64, 64, &pixels);
        let text = String::from_utf8_lossy(&out);
        let opens = text.matches("\x1b_G").count();
        assert!(opens > 1, "expected multiple chunks, got {opens}");
        assert_eq!(text.matches("m=1").count(), opens - 1);
        assert_eq!(text.matches("m=0").count(), 1);
        assert!(text.ends_with("\x1b\\"));
    }

    #[test]
    fn transmit_compresses_flat_canvases_hard() {
        let pixels = [24_u8, 24, 32, 255].repeat(256 * 256);
        let out = kitty_transmit(1, 256, 256, &pixels);
        assert!(out.len() < 4096, "expected tiny output, got {}", out.len());
    }

    #[test]
    fn virtual_placement_transmit_uses_scrollback_keys() {
        let mut seed = 0x9e3779b9u32;
        let pixels: Vec<u8> = (0..64 * 64 * 4)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 24) as u8
            })
            .collect();
        let out = kitty_transmit_placed(77, 64, 64, &pixels, Placement::Cells { cols: 8, rows: 4 });
        let text = String::from_utf8_lossy(&out);
        assert!(text.starts_with("\x1b_Ga=T,f=32,o=z,s=64,v=64,t=d,i=77,U=1,c=8,r=4,q=2,m="));
        assert!(
            !text.contains("C=1"),
            "cursor keys are meaningless for virtual placements"
        );
        assert!(text.ends_with("\x1b\\"));
        assert!(text.matches("\x1b_G").count() > 0);
    }

    #[test]
    fn cursor_placement_transmit_keeps_cursor_keys() {
        let out = kitty_transmit(1, 1, 1, &[0xff, 0x00, 0x00, 0xff]);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("p=1,C=1"),
            "cursor placement keeps the cursor keys"
        );
    }

    #[test]
    fn query_and_named_transmits_carry_the_right_keys() {
        let q = String::from_utf8(kitty_query_medium(
            4,
            "frame-1.png",
            Medium::Named,
            320,
            200,
        ))
        .unwrap();
        assert!(q.starts_with("\x1b_Gi=4,a=q,t=s,f=32,s=320,v=200;"));

        let n = String::from_utf8(kitty_transmit_named(
            4,
            320,
            200,
            "frame-1.png",
            Medium::Named,
            Placement::Cells { cols: 8, rows: 4 },
        ))
        .unwrap();
        assert!(n.starts_with("\x1b_Ga=T,f=32,s=320,v=200,t=s,i=4,U=1,c=8,r=4,q=2;"));
        assert!(n.ends_with("\x1b\\"));
    }

    #[test]
    fn delete_is_deferred_to_a_later_delete_everything() {
        assert_eq!(kitty_delete(5), b"\x1b_Ga=d,d=A,q=2\x1b\\");
    }

    #[test]
    fn placeholder_grid_uses_indexed_foreground_for_small_ids() {
        let out = String::from_utf8(placeholder_grid(7, 1, 1)).unwrap();
        assert!(out.starts_with("\x1b[38;5;7m"));
        assert!(out.ends_with("\x1b[39m"));
    }

    #[test]
    fn placeholder_grid_encodes_id_rows_and_columns() {
        let out = String::from_utf8(placeholder_grid(0x0a0b0c, 3, 2)).unwrap();
        assert!(out.starts_with("\x1b[38;2;10;11;12m"));
        assert!(out.ends_with("\x1b[39m"));
        let row2 = out.find("\x1b[2;1H").unwrap();
        let row1_cells: Vec<char> = out[out.find("\x1b[1;1H").unwrap() + 6..row2]
            .chars()
            .collect();
        assert_eq!(
            row1_cells,
            [
                PLACEHOLDER,
                '\u{0305}',
                '\u{0305}',
                PLACEHOLDER,
                '\u{0305}',
                '\u{030D}',
                PLACEHOLDER,
                '\u{0305}',
                '\u{030E}',
            ]
        );
        let row2_cells: Vec<char> = out[row2 + 6..out.len() - 5].chars().collect();
        assert_eq!(
            row2_cells[1], '\u{030D}',
            "second row uses the next row diacritic"
        );
        assert_eq!(row2_cells.len(), 9);
    }

    #[test]
    fn placeholder_grid_clamps_to_addressable_cells() {
        let out = String::from_utf8(placeholder_grid(1, 1000, 1)).unwrap();
        let cells = out.chars().filter(|&c| c == PLACEHOLDER).count();
        assert_eq!(cells, MAX_PLACEHOLDER_CELLS as usize);
    }
}
