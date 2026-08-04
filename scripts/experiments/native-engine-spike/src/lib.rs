use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratex_layout::{layout, to_display_list, LayoutOptions};
use ratex_parser::parser::parse;
use ratex_render::{render_to_png, RenderOptions};
use ratex_svg::{render_to_svg_with_color_syntax, SvgColorSyntax, SvgOptions};
use ratex_types::color::Color;
use ratex_types::display_item::DisplayItem;
use ratex_types::math_style::MathStyle;
use typst::layout::{Abs, Frame, FrameItem, PagedDocument};
use typst_as_lib::typst_kit_options::TypstKitFontOptions;
use typst_as_lib::TypstEngine;

pub mod golden;

pub const TEXT_SIZE_PT: f64 = 12.0;
const PAGE_WIDTH_PT: f64 = 480.0;
const PAGE_MARGIN_PT: f64 = 8.0;
const BASELINE_MARKER_HEIGHT_PT: f64 = 1.0;
const ALPHA_THRESHOLD: u8 = 8;
const WRAP_REPETITIONS: usize = 12;

#[derive(Clone, Copy)]
struct Probe {
    name: &'static str,
    formula: &'static str,
}

const PROBES: [Probe; 3] = [
    Probe {
        name: "ascender",
        formula: r"\hat{A}^{2^{x}}",
    },
    Probe {
        name: "descender",
        formula: r"\sqrt{y_{j_{q}}}",
    },
    Probe {
        name: "plain",
        formula: "x+y",
    },
];

#[derive(Clone)]
struct MathAsset {
    name: &'static str,
    formula: &'static str,
    standalone_png: [Vec<u8>; 2],
    baseline_png: [Vec<u8>; 2],
    width_pt: f64,
    height_pt: f64,
    depth_pt: f64,
    display_width_em: f64,
    display_height_em: f64,
    display_depth_em: f64,
    layout_width_em: f64,
    layout_height_em: f64,
    layout_depth_em: f64,
    opaque_pixels_dpr1: usize,
}

#[derive(Clone, Debug)]
struct DecodedPng {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
struct OffsetMeasurement {
    offset_px: i32,
    text_baseline_px: f64,
    math_baseline_px: f64,
    marker_rows: Vec<u32>,
}

#[derive(Clone, Debug)]
struct RunSummary {
    offsets: BTreeMap<String, BTreeMap<String, i32>>,
    offset_details: BTreeMap<String, BTreeMap<String, (f64, f64)>>,
    wrap_lines: usize,
    wrap_opaque_ratio: f64,
    standalone_opaque_pixels: usize,
    composed_opaque_pixels: usize,
}

pub fn run_main_spike() -> Result<(), Box<dyn Error>> {
    let summary = run_spike()?;
    println!(
        "AT-3-001 probe complete: {} wrapped lines, {:.4} opaque-pixel ratio.",
        summary.wrap_lines, summary.wrap_opaque_ratio
    );
    for (dpr, probes) in &summary.offsets {
        for (probe, offset) in probes {
            println!("{dpr} {probe}: {offset} px");
        }
    }
    Ok(())
}

fn run_spike() -> Result<RunSummary, Box<dyn Error>> {
    let out_dir = output_dir();
    fs::create_dir_all(&out_dir)?;

    let assets = render_assets(&out_dir)?;
    let mut offsets = BTreeMap::new();
    let mut offset_details = BTreeMap::new();

    for dpr in [1_u32, 2_u32] {
        let mut per_probe = BTreeMap::new();
        let mut per_probe_details = BTreeMap::new();
        for asset in &assets {
            let png = render_baseline_probe(asset, dpr)?;
            fs::write(
                out_dir.join(format!("baseline-{}-dpr{}.png", asset.name, dpr)),
                &png,
            )?;
            let measurement = measure_baseline_offset(&png, dpr)?;
            println!(
                "baseline dpr={} probe={} text={:.2}px math={:.2}px offset={}px rows={:?}",
                dpr,
                asset.name,
                measurement.text_baseline_px,
                measurement.math_baseline_px,
                measurement.offset_px,
                measurement.marker_rows
            );
            per_probe.insert(asset.name.to_string(), measurement.offset_px);
            per_probe_details.insert(
                asset.name.to_string(),
                (measurement.text_baseline_px, measurement.math_baseline_px),
            );
        }
        offsets.insert(format!("dpr{dpr}"), per_probe);
        offset_details.insert(format!("dpr{dpr}"), per_probe_details);
    }

    for dpr in [1_u32, 2_u32] {
        let png = render_mixed_paragraph(&assets, dpr)?;
        fs::write(out_dir.join(format!("mixed-paragraph-dpr{dpr}.png")), png)?;
    }

    let wrap_png = render_wrapping_probe(&assets, 1)?;
    fs::write(out_dir.join("paragraph-wrap-dpr1.png"), &wrap_png)?;
    let wrap_image = decode_png(&wrap_png)?;
    let expected_math_pixels = (0..WRAP_REPETITIONS)
        .map(|index| assets[index % assets.len()].opaque_pixels_dpr1)
        .sum::<usize>();
    let observed_math_pixels = count_math_pixels(&wrap_image);
    let ratio = observed_math_pixels as f64 / expected_math_pixels as f64;
    let line_count = count_marker_lines(&wrap_image);
    println!(
        "wrap lines={} expected_math_pixels={} observed_math_pixels={} ratio={:.6}",
        line_count, expected_math_pixels, observed_math_pixels, ratio
    );

    let summary = RunSummary {
        offsets,
        offset_details,
        wrap_lines: line_count,
        wrap_opaque_ratio: ratio,
        standalone_opaque_pixels: expected_math_pixels,
        composed_opaque_pixels: observed_math_pixels,
    };
    fs::write(
        out_dir.join("summary.json"),
        summary_json(&summary, &assets),
    )?;
    Ok(summary)
}

fn output_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("out")
}

fn render_assets(out_dir: &Path) -> Result<Vec<MathAsset>, Box<dyn Error>> {
    PROBES
        .iter()
        .map(|probe| render_asset(*probe, out_dir))
        .collect()
}

fn render_asset(probe: Probe, out_dir: &Path) -> Result<MathAsset, Box<dyn Error>> {
    let ast = parse(probe.formula)?;
    let layout_options = LayoutOptions {
        style: MathStyle::Text,
        color: Color {
            r: 0.0,
            g: 0.2,
            b: 1.0,
            a: 1.0,
        },
        ..LayoutOptions::default()
    };
    let layout_box = layout(&ast, &layout_options);
    let display_list = to_display_list(&layout_box);

    let svg_options = SvgOptions {
        font_size: TEXT_SIZE_PT,
        padding: 0.0,
        stroke_width: TEXT_SIZE_PT * 0.0375,
        embed_glyphs: false,
        font_dir: String::new(),
    };
    let svg = render_to_svg_with_color_syntax(&display_list, &svg_options, SvgColorSyntax::Rgb)
        .into_bytes();

    let marker_height_em = 0.08;
    let mut baseline_display_list = display_list.clone();
    baseline_display_list.items.push(DisplayItem::Rect {
        x: 0.0,
        y: display_list.height - marker_height_em / 2.0,
        width: display_list.width,
        height: marker_height_em,
        color: Color {
            r: 0.0,
            g: 0.75,
            b: 0.0,
            a: 1.0,
        },
    });
    let mut standalone_pngs = Vec::new();
    let mut baseline_pngs = Vec::new();
    for dpr in [1_u32, 2_u32] {
        let png_options = RenderOptions {
            font_size: TEXT_SIZE_PT as f32,
            padding: 0.0,
            background_color: Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            },
            device_pixel_ratio: dpr as f32,
            ..RenderOptions::default()
        };
        let standalone_png = render_to_png(&display_list, &png_options)?;
        let baseline_png = render_to_png(&baseline_display_list, &png_options)?;
        fs::write(
            out_dir.join(format!("math-{}-standalone-dpr{}.png", probe.name, dpr)),
            &standalone_png,
        )?;
        standalone_pngs.push(standalone_png);
        baseline_pngs.push(baseline_png);
    }
    let standalone_png: [Vec<u8>; 2] = standalone_pngs
        .try_into()
        .map_err(|_| "Expected two standalone PNGs")?;
    let baseline_png: [Vec<u8>; 2] = baseline_pngs
        .try_into()
        .map_err(|_| "Expected two baseline PNGs")?;
    let standalone_image = decode_png(&standalone_png[0])?;
    let opaque_pixels_dpr1 = count_nontransparent_pixels(&standalone_image);

    let svg_name = format!("math-{}.svg", probe.name);
    fs::write(out_dir.join(&svg_name), &svg)?;

    Ok(MathAsset {
        name: probe.name,
        formula: probe.formula,
        standalone_png,
        baseline_png,
        width_pt: display_list.width * TEXT_SIZE_PT,
        height_pt: display_list.height * TEXT_SIZE_PT,
        depth_pt: display_list.depth * TEXT_SIZE_PT,
        display_width_em: display_list.width,
        display_height_em: display_list.height,
        display_depth_em: display_list.depth,
        layout_width_em: layout_box.width,
        layout_height_em: layout_box.height,
        layout_depth_em: layout_box.depth,
        opaque_pixels_dpr1,
    })
}

fn render_baseline_probe(asset: &MathAsset, dpr: u32) -> Result<Vec<u8>, Box<dyn Error>> {
    let source = format!(
        r##"#set page(width: 260pt, height: auto, margin: 8pt, fill: none)
#set text(font: "NewCM10", size: {text_size}pt, fill: black, top-edge: "bounds", bottom-edge: "bounds")
#set par(leading: 0pt)
#let baseline-marker = box(
  width: 12pt,
  height: {marker_height}pt,
  baseline: {marker_height_half}pt,
  fill: rgb("#ff0000"),
)
#let math-box = box(
  width: {width}pt,
  height: {total_height}pt,
  baseline: {depth}pt,
  image("{baseline_png_name}", width: {width}pt, height: {total_height}pt, fit: "stretch"),
)

xxxx #baseline-marker#math-box#baseline-marker xxxx
"##,
        text_size = TEXT_SIZE_PT,
        marker_height = BASELINE_MARKER_HEIGHT_PT,
        marker_height_half = BASELINE_MARKER_HEIGHT_PT / 2.0,
        width = asset.width_pt,
        total_height = asset.height_pt + asset.depth_pt,
        depth = asset.depth_pt,
        baseline_png_name = baseline_png_name(asset, dpr),
    );
    render_typst(&source, std::slice::from_ref(asset), dpr as f32)
}

fn render_mixed_paragraph(assets: &[MathAsset], dpr: u32) -> Result<Vec<u8>, Box<dyn Error>> {
    let boxes = assets
        .iter()
        .map(|asset| math_box_source(asset, dpr))
        .collect::<Vec<_>>()
        .join("\n");
    let source = format!(
        r#"#set page(width: {page_width}pt, height: auto, margin: {margin}pt, fill: none)
#set text(font: "NewCM10", size: {text_size}pt, fill: black, top-edge: "bounds", bottom-edge: "bounds")
#set par(leading: 4pt)
{boxes}

The value x #math_ascender x of the ascender probe, x #math_descender x of the descender probe, and x #math_plain x of the plain probe are embedded as inline RaTeX boxes inside one Typst paragraph.
"#,
        page_width = PAGE_WIDTH_PT,
        margin = PAGE_MARGIN_PT,
        text_size = TEXT_SIZE_PT,
    );
    render_typst(&source, assets, dpr as f32)
}

fn render_wrapping_probe(assets: &[MathAsset], dpr: u32) -> Result<Vec<u8>, Box<dyn Error>> {
    let boxes = assets
        .iter()
        .map(|asset| math_box_source(asset, dpr))
        .collect::<Vec<_>>()
        .join("\n");
    let mut paragraph = String::new();
    for index in 0..WRAP_REPETITIONS {
        let asset = &assets[index % assets.len()];
        paragraph.push_str(&format!(
            "word #math_{name}#wrap_marker ",
            name = asset.name
        ));
    }
    let source = format!(
        r##"#set page(width: {page_width}pt, height: auto, margin: {margin}pt, fill: none)
#set text(font: "NewCM10", size: {text_size}pt, fill: black, top-edge: "bounds", bottom-edge: "bounds")
#set par(leading: 4pt)
#let wrap_marker = box(
  width: 4pt,
  height: {marker_height}pt,
  baseline: {marker_height_half}pt,
  fill: rgb("#ff0000"),
)
{boxes}

{paragraph}
"##,
        page_width = PAGE_WIDTH_PT,
        margin = PAGE_MARGIN_PT,
        text_size = TEXT_SIZE_PT,
        marker_height = BASELINE_MARKER_HEIGHT_PT,
        marker_height_half = BASELINE_MARKER_HEIGHT_PT / 2.0,
    );
    render_typst(&source, assets, dpr as f32)
}

fn math_box_source(asset: &MathAsset, dpr: u32) -> String {
    format!(
        r#"#let math_{name} = box(
  width: {width}pt,
  height: {total_height}pt,
  baseline: {depth}pt,
  image("{png_name}", width: {width}pt, height: {total_height}pt, fit: "stretch"),
)"#,
        name = asset.name,
        width = asset.width_pt,
        total_height = asset.height_pt + asset.depth_pt,
        depth = asset.depth_pt,
        png_name = standalone_png_name(asset, dpr),
    )
}

fn render_typst(
    source: &str,
    assets: &[MathAsset],
    pixel_per_pt: f32,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let static_files = assets
        .iter()
        .flat_map(|asset| {
            [
                (
                    standalone_png_name(asset, pixel_per_pt as u32),
                    asset.standalone_png[dpr_index(pixel_per_pt as u32)].clone(),
                ),
                (
                    baseline_png_name(asset, pixel_per_pt as u32),
                    asset.baseline_png[dpr_index(pixel_per_pt as u32)].clone(),
                ),
            ]
        })
        .collect::<Vec<_>>();
    let static_file_refs = static_files
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect::<Vec<_>>();
    let engine = TypstEngine::builder()
        .main_file(source.to_string())
        .with_static_file_resolver(static_file_refs)
        .search_fonts_with(embedded_font_options().typst_kit_options())
        .build();
    let document: PagedDocument = engine.compile().output?;
    let pixmap = typst_render::render_merged(&document, pixel_per_pt, Abs::zero(), None);
    Ok(pixmap.encode_png()?)
}

fn standalone_png_name(asset: &MathAsset, dpr: u32) -> String {
    format!("math-{}-dpr{}.png", asset.name, dpr)
}

fn baseline_png_name(asset: &MathAsset, dpr: u32) -> String {
    format!("math-{}-baseline-dpr{}.png", asset.name, dpr)
}

fn dpr_index(dpr: u32) -> usize {
    match dpr {
        1 => 0,
        2 => 1,
        _ => panic!("The spike only supports dpr 1 and 2"),
    }
}

fn decode_png(bytes: &[u8]) -> Result<DecodedPng, Box<dyn Error>> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or("PNG output buffer size is unavailable")?
    ];
    let info = reader.next_frame(&mut buffer)?;
    let bytes = &buffer[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => bytes.to_vec(),
        png::ColorType::Rgb => bytes
            .chunks_exact(3)
            .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => bytes
            .chunks_exact(2)
            .flat_map(|ga| [ga[0], ga[0], ga[0], ga[1]])
            .collect(),
        png::ColorType::Grayscale => bytes
            .iter()
            .flat_map(|value| [*value, *value, *value, 255])
            .collect(),
        png::ColorType::Indexed => {
            return Err("Indexed PNG remained indexed after expansion".into());
        }
    };
    Ok(DecodedPng {
        width: info.width,
        height: info.height,
        rgba,
    })
}

fn measure_baseline_offset(bytes: &[u8], dpr: u32) -> Result<OffsetMeasurement, Box<dyn Error>> {
    let image = decode_png(bytes)?;
    let mut row_counts = vec![0_usize; image.height as usize];
    for y in 0..image.height {
        for x in 0..image.width {
            let [r, g, b, a] = pixel(&image, x, y);
            if a > ALPHA_THRESHOLD && r > 180 && g < 100 && b < 100 {
                row_counts[y as usize] += 1;
            }
        }
    }
    let minimum_marker_pixels = (8 * dpr) as usize;
    let marker_rows = row_counts
        .iter()
        .enumerate()
        .filter_map(|(row, count)| (*count >= minimum_marker_pixels).then_some(row as u32))
        .collect::<Vec<_>>();
    if marker_rows.is_empty() {
        return Err("No baseline marker rows were found".into());
    }
    let text_baseline_px =
        marker_rows.iter().map(|row| *row as f64).sum::<f64>() / marker_rows.len() as f64;
    let mut math_row_counts = vec![0_usize; image.height as usize];
    for y in 0..image.height {
        for x in 0..image.width {
            let [r, g, b, a] = pixel(&image, x, y);
            if a > ALPHA_THRESHOLD && g > 120 && r < 100 && b < 100 {
                math_row_counts[y as usize] += 1;
            }
        }
    }
    let minimum_math_marker_pixels = (2 * dpr) as usize;
    let math_rows = math_row_counts
        .iter()
        .enumerate()
        .filter_map(|(row, count)| (*count >= minimum_math_marker_pixels).then_some(row as u32))
        .collect::<Vec<_>>();
    if math_rows.is_empty() {
        return Err("No RaTeX baseline marker rows were found".into());
    }
    let math_baseline_px =
        math_rows.iter().map(|row| *row as f64).sum::<f64>() / math_rows.len() as f64;
    Ok(OffsetMeasurement {
        offset_px: (math_baseline_px - text_baseline_px).round() as i32,
        text_baseline_px,
        math_baseline_px,
        marker_rows,
    })
}

fn count_nontransparent_pixels(image: &DecodedPng) -> usize {
    image
        .rgba
        .chunks_exact(4)
        .filter(|rgba| rgba[3] > ALPHA_THRESHOLD)
        .count()
}

fn count_math_pixels(image: &DecodedPng) -> usize {
    image
        .rgba
        .chunks_exact(4)
        .filter(|rgba| {
            let r = rgba[0];
            let g = rgba[1];
            let b = rgba[2];
            let a = rgba[3];
            a > ALPHA_THRESHOLD && b > 120 && b > r.saturating_add(30) && b > g.saturating_add(30)
        })
        .count()
}

fn count_marker_lines(image: &DecodedPng) -> usize {
    let mut active_rows = Vec::new();
    for y in 0..image.height {
        let red_pixels = (0..image.width)
            .filter(|x| {
                let [r, g, b, a] = pixel(image, *x, y);
                a > ALPHA_THRESHOLD && r > 180 && g < 100 && b < 100
            })
            .count();
        if red_pixels >= 3 {
            active_rows.push(y);
        }
    }
    group_consecutive(&active_rows).len()
}

fn group_consecutive(rows: &[u32]) -> Vec<Vec<u32>> {
    let mut groups: Vec<Vec<u32>> = Vec::new();
    for row in rows {
        match groups.last_mut() {
            Some(group) if group.last().copied() == Some(row.saturating_sub(1)) => {
                group.push(*row);
            }
            _ => groups.push(vec![*row]),
        }
    }
    groups
}

fn pixel(image: &DecodedPng, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * image.width + x) * 4) as usize;
    [
        image.rgba[index],
        image.rgba[index + 1],
        image.rgba[index + 2],
        image.rgba[index + 3],
    ]
}

fn summary_json(summary: &RunSummary, assets: &[MathAsset]) -> String {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str(&format!("  \"text_size_pt\": {:.1},\n", TEXT_SIZE_PT));
    json.push_str("  \"ratex_unit_to_pt\": \"1em = 12pt\",\n");
    json.push_str("  \"offsets_px\": {\n");
    for (dpr_index, (dpr, probes)) in summary.offsets.iter().enumerate() {
        json.push_str(&format!("    \"{dpr}\": {{\n"));
        for (probe_index, (probe, offset)) in probes.iter().enumerate() {
            let details = summary
                .offset_details
                .get(dpr)
                .and_then(|values| values.get(probe))
                .copied()
                .unwrap_or((0.0, 0.0));
            json.push_str(&format!(
                "      \"{probe}\": {{ \"offset\": {offset}, \"text_baseline\": {:.3}, \"math_baseline\": {:.3} }}{}",
                details.0,
                details.1,
                if probe_index + 1 == probes.len() { "\n" } else { ",\n" }
            ));
        }
        json.push_str(&format!(
            "    }}{}",
            if dpr_index + 1 == summary.offsets.len() {
                "\n"
            } else {
                ",\n"
            }
        ));
    }
    json.push_str("  },\n");
    json.push_str("  \"probes\": {\n");
    for (index, asset) in assets.iter().enumerate() {
        json.push_str(&format!(
            "    \"{}\": {{ \"formula\": \"{}\", \"layout\": {{ \"width_em\": {:.6}, \"height_em\": {:.6}, \"depth_em\": {:.6} }}, \"display_list\": {{ \"width_em\": {:.6}, \"height_em\": {:.6}, \"depth_em\": {:.6} }}, \"box\": {{ \"width_pt\": {:.6}, \"height_pt\": {:.6}, \"depth_pt\": {:.6} }}, \"standalone_opaque_pixels_dpr1\": {} }}{}",
            asset.name,
            json_escape(asset.formula),
            asset.layout_width_em,
            asset.layout_height_em,
            asset.layout_depth_em,
            asset.display_width_em,
            asset.display_height_em,
            asset.display_depth_em,
            asset.width_pt,
            asset.height_pt,
            asset.depth_pt,
            asset.opaque_pixels_dpr1,
            if index + 1 == assets.len() { "\n" } else { ",\n" }
        ));
    }
    json.push_str("  },\n");
    json.push_str(&format!(
        "  \"wrapping\": {{ \"box_count\": {}, \"line_count\": {}, \"standalone_opaque_pixels\": {}, \"composed_opaque_pixels\": {}, \"opaque_ratio\": {:.6} }}\n",
        WRAP_REPETITIONS,
        summary.wrap_lines,
        summary.standalone_opaque_pixels,
        summary.composed_opaque_pixels,
        summary.wrap_opaque_ratio
    ));
    json.push_str("}\n");
    json
}

fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[derive(Clone, Copy, Debug)]
pub struct EmbeddedFontOptions {
    include_system_fonts: bool,
    include_embedded_fonts: bool,
    font_faces: usize,
}

impl EmbeddedFontOptions {
    pub fn font_faces(self) -> usize {
        self.font_faces
    }

    pub fn typst_kit_options(self) -> TypstKitFontOptions {
        debug_assert!(
            !self.include_system_fonts,
            "The native spike must not scan system font directories"
        );
        debug_assert!(
            self.include_embedded_fonts,
            "The native spike must load fonts embedded in the binary"
        );
        TypstKitFontOptions::default()
            .include_system_fonts(self.include_system_fonts)
            .include_dirs(std::iter::empty::<PathBuf>())
            .include_embedded_fonts(self.include_embedded_fonts)
    }
}

pub fn embedded_font_options() -> EmbeddedFontOptions {
    EmbeddedFontOptions {
        include_system_fonts: false,
        include_embedded_fonts: true,
        // typst-assets 0.13.1 embeds 17 individual font files/faces.
        font_faces: 17,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ColdStartMetrics {
    pub engine_build: Duration,
    pub first_render: Duration,
    pub png_bytes: usize,
    pub fonts_loaded: usize,
}

pub fn render_cold_start_block(process_start: Instant) -> Result<ColdStartMetrics, Box<dyn Error>> {
    const INLINE_FORMULA: &str = r"\frac{a+b}{c}";
    const DISPLAY_FORMULA: &str = r"\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}";
    const DPR: f32 = 2.0;

    let engine_build_started = process_start;
    let inline = render_cold_start_asset("inline", INLINE_FORMULA, MathStyle::Text, DPR)?;
    let display = render_cold_start_asset("display", DISPLAY_FORMULA, MathStyle::Display, DPR)?;
    let assets = [inline, display];
    let source = cold_start_source(&assets[0], &assets[1]);
    let static_files = assets
        .iter()
        .map(|asset| (asset.png_name.as_str(), asset.png.as_slice()))
        .collect::<Vec<_>>();

    let engine = TypstEngine::builder()
        .main_file(source)
        .with_static_file_resolver(static_files)
        .search_fonts_with(embedded_font_options().typst_kit_options())
        .build();
    let engine_build = engine_build_started.elapsed();

    let first_render_started = Instant::now();
    let document: PagedDocument = engine.compile().output?;
    let used_font_faces = count_document_font_faces(&document);
    assert!(
        used_font_faces > 0,
        "The rendered document must use embedded fonts"
    );
    let fonts_loaded = embedded_font_options().font_faces();
    assert!(fonts_loaded > 0, "The engine must have embedded font faces");
    let pixmap = typst_render::render_merged(&document, DPR, Abs::zero(), None);
    let png = pixmap.encode_png()?;
    let first_render = first_render_started.elapsed();
    debug_assert!(
        process_start.elapsed() >= engine_build + first_render,
        "The total process timer must contain the measured phases"
    );

    Ok(ColdStartMetrics {
        engine_build,
        first_render,
        png_bytes: png.len(),
        fonts_loaded,
    })
}

struct ColdStartAsset {
    png_name: String,
    png: Vec<u8>,
    width_pt: f64,
    total_height_pt: f64,
    depth_pt: f64,
}

fn render_cold_start_asset(
    name: &str,
    formula: &str,
    style: MathStyle,
    dpr: f32,
) -> Result<ColdStartAsset, Box<dyn Error>> {
    let ast = parse(formula)?;
    let layout_box = layout(
        &ast,
        &LayoutOptions {
            style,
            ..LayoutOptions::default()
        },
    );
    let display_list = to_display_list(&layout_box);
    let png = render_to_png(
        &display_list,
        &RenderOptions {
            font_size: TEXT_SIZE_PT as f32,
            padding: 0.0,
            background_color: Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            },
            device_pixel_ratio: dpr,
            ..RenderOptions::default()
        },
    )?;
    Ok(ColdStartAsset {
        png_name: format!("coldstart-{name}.png"),
        png,
        width_pt: display_list.width * TEXT_SIZE_PT,
        total_height_pt: (display_list.height + display_list.depth) * TEXT_SIZE_PT,
        depth_pt: display_list.depth * TEXT_SIZE_PT,
    })
}

fn cold_start_source(inline: &ColdStartAsset, display: &ColdStartAsset) -> String {
    format!(
        r#"#set page(width: 360pt, height: auto, margin: 8pt, fill: none)
#set text(font: "NewCM10", size: {text_size}pt, fill: black, top-edge: "bounds", bottom-edge: "bounds")
#set par(leading: 4pt)
#let inline_math = box(
  width: {inline_width}pt,
  height: {inline_height}pt,
  baseline: {inline_depth}pt,
  image("{inline_png}", width: {inline_width}pt, height: {inline_height}pt, fit: "stretch"),
)
#let display_math = box(
  width: {display_width}pt,
  height: {display_height}pt,
  baseline: {display_depth}pt,
  image("{display_png}", width: {display_width}pt, height: {display_height}pt, fit: "stretch"),
)

*Native cold start* renders one inline formula #inline_math inside a short paragraph.

#align(center, display_math)
"#,
        text_size = TEXT_SIZE_PT,
        inline_width = inline.width_pt,
        inline_height = inline.total_height_pt,
        inline_depth = inline.depth_pt,
        inline_png = inline.png_name,
        display_width = display.width_pt,
        display_height = display.total_height_pt,
        display_depth = display.depth_pt,
        display_png = display.png_name,
    )
}

fn count_document_font_faces(document: &PagedDocument) -> usize {
    let mut faces = BTreeSet::new();
    for page in &document.pages {
        collect_frame_font_faces(&page.frame, &mut faces);
    }
    faces.len()
}

fn collect_frame_font_faces(frame: &Frame, faces: &mut BTreeSet<(String, String)>) {
    for (_, item) in frame.items() {
        match item {
            FrameItem::Text(text) => {
                let info = text.font.info();
                faces.insert((info.family.to_string(), format!("{:?}", info.variant)));
            }
            FrameItem::Group(group) => collect_frame_font_faces(&group.frame, faces),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("The test lock should not be poisoned")
    }

    #[test]
    fn baseline_offset_dpr1() {
        let _guard = test_guard();
        let summary = run_spike().expect("The dpr 1 spike should render and measure");
        let offsets = summary.offsets.get("dpr1").expect("dpr1 offsets");
        let distinct_baselines = PROBES
            .iter()
            .map(|probe| {
                summary
                    .offset_details
                    .get("dpr1")
                    .and_then(|details| details.get(probe.name))
                    .expect("probe baseline details")
                    .0 as i32
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            distinct_baselines.len() >= 2,
            "The measurement must locate per-probe marker rows, not use one fixed baseline"
        );
        for probe in PROBES {
            let offset = offsets.get(probe.name).expect("probe offset");
            assert!(
                offset.abs() <= 1,
                "{} baseline offset was {} px at dpr 1",
                probe.name,
                offset
            );
        }
    }

    #[test]
    fn baseline_offset_dpr2() {
        let _guard = test_guard();
        let summary = run_spike().expect("The dpr 2 spike should render and measure");
        let offsets = summary.offsets.get("dpr2").expect("dpr2 offsets");
        for probe in PROBES {
            let offset = offsets.get(probe.name).expect("probe offset");
            assert!(
                offset.abs() <= 2,
                "{} baseline offset was {} px at dpr 2",
                probe.name,
                offset
            );
        }
    }

    #[test]
    fn paragraph_wraps_without_clipping() {
        let _guard = test_guard();
        let summary = run_spike().expect("The wrapping spike should render and measure");
        assert!(
            summary.wrap_lines >= 2,
            "The paragraph produced only {} measured line(s)",
            summary.wrap_lines
        );
        assert!(
            summary.wrap_opaque_ratio >= 0.95,
            "Only {:.2}% of standalone math pixels survived composition",
            summary.wrap_opaque_ratio * 100.0
        );
    }
}
