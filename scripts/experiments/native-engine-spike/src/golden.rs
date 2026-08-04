use std::error::Error;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use ratex_layout::{layout, to_display_list, LayoutOptions};
use ratex_parser::parser::parse;
use ratex_render::{render_to_png, RenderOptions};
use ratex_types::color::Color;
use ratex_types::math_style::MathStyle;
use serde_json::{json, Value};
use typst::layout::{Abs, PagedDocument};
use typst_as_lib::TypstEngine;

use crate::{embedded_font_options, TEXT_SIZE_PT};

const DPR: f32 = 2.0;
const FORMULA_GAP_PX: u32 = 8;
const V2_TIMEOUT: Duration = Duration::from_secs(20);
const PAIR_BACKGROUND: [u8; 4] = [0x0d, 0x11, 0x17, 0xff];
const PAIR_SEPARATOR: [u8; 4] = [0x58, 0x68, 0x69, 0xff];
const ERROR_IMAGE_WIDTH: u32 = 240;
const ERROR_IMAGE_HEIGHT: u32 = 80;

const DOCUMENT_SOURCE: &str = r#"# Golden document sample

This paragraph has **bold text** and inline math $E=mc^2$.

- First item
- Second item

| Name | Value |
| --- | ---: |
| Alpha | 1 |
| Beta | 2 |

```rust
fn main() {
    println!("hello");
}
```"#;

const DOCUMENT_TYPST_SOURCE: &str = r####"#set page(width: 420pt, height: auto, margin: 12pt, fill: none)
#set text(font: "NewCM10", size: 12pt, fill: rgb("#e6edf3"))
#set par(leading: 4pt)
#let inline_math = box(
  width: INLINE_WIDTHpt,
  height: INLINE_HEIGHTpt,
  baseline: INLINE_DEPTHpt,
  image("document-inline-math.png", width: INLINE_WIDTHpt, height: INLINE_HEIGHTpt, fit: "stretch"),
)

= Golden document sample

This paragraph has *bold text* and inline math #inline_math.

- First item
- Second item

#table(
  columns: (1fr, auto),
  inset: 4pt,
  stroke: 0.5pt,
  table.header([*Name*], [*Value*]),
  [Alpha], [1],
  [Beta], [2],
)

```rust
fn main() {
    println!("hello");
}
```
"####;

const COVERAGE_PROBES: [(&str, &str); 22] = [
    (
        "probe-01-align",
        r"\begin{align*} a &= b + c \\ d &= e \end{align*}",
    ),
    (
        "probe-02-gather",
        r"\begin{gather*} x = 1 \\ y = 2 \end{gather*}",
    ),
    (
        "probe-03-cases",
        r"f(x)=\begin{cases} 1 & x>0 \\ 0 & x\le 0\end{cases}",
    ),
    (
        "probe-04-array",
        r"\begin{array}{cc} a & b \\ c & d \end{array}",
    ),
    (
        "probe-05-alphabets",
        r"\mathbb{R}\ \mathcal{L}\ \mathfrak{g}",
    ),
    ("probe-06-text", r"\text{if } x > 0 \text{ then}"),
    ("probe-07-binom", r"\binom{n}{k}"),
    ("probe-08-overbrace", r"\overbrace{a+b}^{\text{sum}}"),
    ("probe-09-stackrel", r"A \stackrel{f}{\to} B"),
    ("probe-10-substack", r"\sum_{\substack{i<n \\ j<m}} a_{ij}"),
    ("probe-11-big-delimiters", r"\Bigl( \frac{a}{b} \Bigr)"),
    ("probe-12-xrightarrow", r"A \xrightarrow{\varphi} B"),
    ("probe-13-textcolor", r"\textcolor{red}{x} + y"),
    ("probe-14-mhchem", r"\ce{H2SO4}"),
    ("probe-15-physical-units", r"\pu{3.5 kg m/s}"),
    ("probe-16-cancel", r"\cancel{x} + y"),
    ("probe-17-boxed", r"\boxed{E=mc^2}"),
    (
        "probe-18-equation-tag",
        r"\begin{equation} a=b \tag{1} \end{equation}",
    ),
    ("probe-19-cjk-text", r"\text{日本語のテキスト}"),
    (
        "probe-20-nested-fraction",
        r"\frac{1}{1+\frac{1}{1+\frac{1}{x}}}",
    ),
    (
        "probe-21-product",
        r"\prod_{i=1}^{n}\left(1-\frac{1}{i}\right)",
    ),
    (
        "probe-22-vmatrix",
        r"\begin{vmatrix} a & b \\ c & d \end{vmatrix}",
    ),
];

#[derive(Clone, Debug)]
struct FormulaInput {
    latex: String,
    display: bool,
}

#[derive(Clone, Debug)]
enum CaseInput {
    Formulas(Vec<FormulaInput>),
    Document {
        markdown: String,
        typst_source: &'static str,
    },
}

#[derive(Clone, Debug)]
struct GoldenCase {
    id: String,
    kind: String,
    input: CaseInput,
}

#[derive(Clone, Debug)]
pub struct EngineResult {
    pub status: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: Option<usize>,
    pub elapsed_ms: u128,
    png: Option<Vec<u8>>,
}

impl EngineResult {
    pub fn is_ok(&self) -> bool {
        self.status == "ok"
    }
}

#[derive(Clone, Debug)]
pub struct GoldenCaseResult {
    pub id: String,
    pub kind: String,
    pub v2: EngineResult,
    pub native: EngineResult,
}

#[derive(Clone, Debug)]
pub struct GoldenRun {
    pub output_dir: PathBuf,
    pub cases: Vec<GoldenCaseResult>,
}

impl GoldenRun {
    pub fn summary_line(&self) -> String {
        let v2_ok = self.cases.iter().filter(|case| case.v2.is_ok()).count();
        let native_ok = self.cases.iter().filter(|case| case.native.is_ok()).count();
        let both_ok = self
            .cases
            .iter()
            .filter(|case| case.v2.is_ok() && case.native.is_ok())
            .count();
        format!(
            "golden: {} cases, v2 ok {}, native ok {}, both ok {}",
            self.cases.len(),
            v2_ok,
            native_ok,
            both_ok
        )
    }
}

#[derive(Clone, Debug)]
struct DecodedPng {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

pub fn run_golden() -> Result<GoldenRun, Box<dyn Error>> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repository_root = manifest_dir
        .ancestors()
        .nth(3)
        .ok_or("The repository root could not be derived")?;
    run_golden_at(repository_root, &manifest_dir.join("out/golden"))
}

pub fn run_golden_at(
    repository_root: &Path,
    output_dir: &Path,
) -> Result<GoldenRun, Box<dyn Error>> {
    if output_dir.exists() {
        fs::remove_dir_all(output_dir)?;
    }
    fs::create_dir_all(output_dir)?;

    let cases = load_cases(repository_root)?;
    let mut index_cases = Vec::with_capacity(cases.len());
    let mut results = Vec::with_capacity(cases.len());

    for case in cases {
        let v2 = render_v2_case(repository_root, &case);
        let native = render_native_case(&case);

        write_engine_image(output_dir, &case.id, "v2", &v2)?;
        write_engine_image(output_dir, &case.id, "native", &native)?;
        write_pair_image(output_dir, &case.id, &v2, &native)?;

        index_cases.push(case_json(&case, &v2, &native));
        results.push(GoldenCaseResult {
            id: case.id,
            kind: case.kind,
            v2,
            native,
        });
    }

    let index = json!({
        "schemaVersion": 1,
        "cases": index_cases,
    });
    fs::write(
        output_dir.join("index.json"),
        serde_json::to_vec_pretty(&index)?,
    )?;

    Ok(GoldenRun {
        output_dir: output_dir.to_path_buf(),
        cases: results,
    })
}

fn load_cases(repository_root: &Path) -> Result<Vec<GoldenCase>, Box<dyn Error>> {
    let corpus_path = repository_root.join("tests/fixtures/renderer/formula-corpus.json");
    let corpus: Value = serde_json::from_slice(&fs::read(corpus_path)?)?;
    let valid_cases = corpus
        .get("validCases")
        .and_then(Value::as_array)
        .ok_or("formula-corpus.json does not contain validCases")?;

    let mut cases = Vec::with_capacity(valid_cases.len() + COVERAGE_PROBES.len() + 1);
    for value in valid_cases {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or("A valid corpus case does not contain an id")?;
        let formulas = value
            .get("formulas")
            .and_then(Value::as_array)
            .ok_or("A valid corpus case does not contain formulas")?
            .iter()
            .map(|formula| {
                Ok(FormulaInput {
                    latex: formula
                        .get("latex")
                        .and_then(Value::as_str)
                        .ok_or("A corpus formula does not contain latex")?
                        .to_string(),
                    display: formula
                        .get("display")
                        .and_then(Value::as_bool)
                        .ok_or("A corpus formula does not contain display")?,
                })
            })
            .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
        cases.push(GoldenCase {
            id: id.to_string(),
            kind: "corpus".to_string(),
            input: CaseInput::Formulas(formulas),
        });
    }

    cases.extend(COVERAGE_PROBES.iter().map(|(id, latex)| GoldenCase {
        id: (*id).to_string(),
        kind: "probe".to_string(),
        input: CaseInput::Formulas(vec![FormulaInput {
            latex: (*latex).to_string(),
            display: true,
        }]),
    }));
    cases.push(GoldenCase {
        id: "document-markdown".to_string(),
        kind: "document".to_string(),
        input: CaseInput::Document {
            markdown: DOCUMENT_SOURCE.to_string(),
            typst_source: DOCUMENT_TYPST_SOURCE,
        },
    });
    Ok(cases)
}

fn render_v2_case(repository_root: &Path, case: &GoldenCase) -> EngineResult {
    let started = Instant::now();
    let request = match &case.input {
        CaseInput::Formulas(formulas) => json!({
            "protocol": "tmath-render/1",
            "kind": "formulas",
            "formulas": formulas
                .iter()
                .map(|formula| json!({
                    "latex": formula.latex,
                    "display": formula.display,
                }))
                .collect::<Vec<_>>(),
            "options": {
                "layout": {
                    "deviceScaleFactor": 2
                }
            }
        }),
        CaseInput::Document { markdown, .. } => json!({
            "protocol": "tmath-render/1",
            "kind": "document",
            "text": markdown,
            "options": {
                "layout": {
                    "deviceScaleFactor": 2
                }
            }
        }),
    };

    match invoke_v2(repository_root, &request) {
        Ok(png) => match decode_png(&png) {
            Ok(decoded) => EngineResult {
                status: "ok".to_string(),
                width: Some(decoded.width),
                height: Some(decoded.height),
                bytes: Some(png.len()),
                elapsed_ms: started.elapsed().as_millis(),
                png: Some(png),
            },
            Err(error) => error_result(format!("png_decode: {error}"), started),
        },
        Err(error) => error_result(error, started),
    }
}

fn invoke_v2(repository_root: &Path, request: &Value) -> Result<Vec<u8>, String> {
    let mut child = Command::new("node")
        .arg("dist/renderer/subprocess.js")
        .current_dir(repository_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn: {error}"))?;

    let mut stdin = child.stdin.take().ok_or("stdin_unavailable")?;
    let request_bytes =
        serde_json::to_vec(request).map_err(|error| format!("request_json: {error}"))?;
    stdin
        .write_all(&request_bytes)
        .map_err(|error| format!("stdin_write: {error}"))?;
    drop(stdin);

    let mut stdout = child.stdout.take().ok_or("stdout_unavailable")?;
    let mut stderr = child.stderr.take().ok_or("stderr_unavailable")?;
    let stdout_thread = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr_thread = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });

    let deadline = Instant::now() + V2_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Err("timeout".to_string());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Err(format!("wait: {error}"));
            }
        }
    };
    let stdout_bytes = stdout_thread
        .join()
        .map_err(|_| "stdout_reader_panicked".to_string())?
        .map_err(|error| format!("stdout_read: {error}"))?;
    let _stderr_bytes = stderr_thread
        .join()
        .map_err(|_| "stderr_reader_panicked".to_string())?
        .map_err(|error| format!("stderr_read: {error}"))?;

    if !status.success() {
        return Err(format!(
            "process_exit: {}",
            status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string())
        ));
    }

    let response: Value =
        serde_json::from_slice(&stdout_bytes).map_err(|error| format!("response_json: {error}"))?;
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(response
            .pointer("/error/code")
            .and_then(Value::as_str)
            .unwrap_or("renderer_error")
            .to_string());
    }
    if response.get("protocol").and_then(Value::as_str) != Some("tmath-render/1") {
        return Err("protocol_mismatch".to_string());
    }
    let base64 = response
        .get("base64")
        .and_then(Value::as_str)
        .ok_or("base64_missing")?;
    decode_base64(base64)
}

fn render_native_case(case: &GoldenCase) -> EngineResult {
    let started = Instant::now();
    let rendered = match &case.input {
        CaseInput::Formulas(formulas) => render_native_formulas(formulas),
        CaseInput::Document { typst_source, .. } => render_native_document(typst_source),
    };
    match rendered {
        Ok(png) => match decode_png(&png) {
            Ok(decoded) => EngineResult {
                status: "ok".to_string(),
                width: Some(decoded.width),
                height: Some(decoded.height),
                bytes: Some(png.len()),
                elapsed_ms: started.elapsed().as_millis(),
                png: Some(png),
            },
            Err(error) => error_result(format!("png_decode: {error}"), started),
        },
        Err(error) => error_result(error.to_string(), started),
    }
}

fn render_native_formulas(formulas: &[FormulaInput]) -> Result<Vec<u8>, Box<dyn Error>> {
    if formulas.is_empty() {
        return Err("The formula case is empty".into());
    }
    let mut images = Vec::with_capacity(formulas.len());
    for formula in formulas {
        let ast = parse(&formula.latex)?;
        let display_list = to_display_list(&layout(
            &ast,
            &LayoutOptions {
                style: if formula.display {
                    MathStyle::Display
                } else {
                    MathStyle::Text
                },
                // Match the V2 renderer's fixed dark-theme text color (#e6edf3) so
                // the side-by-side composites are comparable on the dark backdrop.
                color: Color {
                    r: 0.902,
                    g: 0.929,
                    b: 0.953,
                    a: 1.0,
                },
                ..LayoutOptions::default()
            },
        ));
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
                device_pixel_ratio: DPR,
                ..RenderOptions::default()
            },
        )?;
        images.push(decode_png(&png)?);
    }
    encode_png(&stack_images(&images, FORMULA_GAP_PX)?)
}

fn render_native_document(typst_source: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let formula = FormulaInput {
        latex: "E=mc^2".to_string(),
        display: false,
    };
    let math_png = render_native_formulas(&[formula])?;
    let math = decode_png(&math_png)?;
    let width_pt = math.width as f64 / DPR as f64;
    let height_pt = math.height as f64 / DPR as f64;
    let ast = parse("E=mc^2")?;
    let display_list = to_display_list(&layout(
        &ast,
        &LayoutOptions {
            style: MathStyle::Text,
            ..LayoutOptions::default()
        },
    ));
    let depth_pt = display_list.depth * TEXT_SIZE_PT;
    let source = typst_source
        .replace("INLINE_WIDTH", &format!("{width_pt:.6}"))
        .replace("INLINE_HEIGHT", &format!("{height_pt:.6}"))
        .replace("INLINE_DEPTH", &format!("{depth_pt:.6}"));
    let static_files = [("document-inline-math.png", math_png.as_slice())];
    let engine = TypstEngine::builder()
        .main_file(source)
        .with_static_file_resolver(static_files)
        .search_fonts_with(embedded_font_options().typst_kit_options())
        .build();
    let document: PagedDocument = engine.compile().output?;
    let pixmap = typst_render::render_merged(&document, DPR, Abs::zero(), None);
    Ok(pixmap.encode_png()?)
}

fn error_result(status: String, started: Instant) -> EngineResult {
    EngineResult {
        status,
        width: None,
        height: None,
        bytes: None,
        elapsed_ms: started.elapsed().as_millis(),
        png: None,
    }
}

fn case_json(case: &GoldenCase, v2: &EngineResult, native: &EngineResult) -> Value {
    let mut value = json!({
        "id": case.id,
        "kind": case.kind,
        "v2": engine_json(v2),
        "native": engine_json(native),
    });
    match &case.input {
        CaseInput::Formulas(formulas) => {
            value["latex"] = Value::Array(
                formulas
                    .iter()
                    .map(|formula| Value::String(formula.latex.clone()))
                    .collect(),
            );
        }
        CaseInput::Document { markdown, .. } => {
            value["source"] = Value::String(markdown.clone());
        }
    }
    value
}

fn engine_json(result: &EngineResult) -> Value {
    json!({
        "status": result.status,
        "width": result.width,
        "height": result.height,
        "bytes": result.bytes,
        "elapsed_ms": result.elapsed_ms,
    })
}

fn write_engine_image(
    output_dir: &Path,
    case_id: &str,
    engine: &str,
    result: &EngineResult,
) -> Result<(), Box<dyn Error>> {
    let bytes = match &result.png {
        Some(png) => png.clone(),
        None => encode_png(&error_placeholder())?,
    };
    fs::write(output_dir.join(format!("{case_id}-{engine}.png")), bytes)?;
    Ok(())
}

fn write_pair_image(
    output_dir: &Path,
    case_id: &str,
    v2: &EngineResult,
    native: &EngineResult,
) -> Result<(), Box<dyn Error>> {
    let left = match &v2.png {
        Some(png) => decode_png(png)?,
        None => error_placeholder(),
    };
    let right = match &native.png {
        Some(png) => decode_png(png)?,
        None => error_placeholder(),
    };
    let width = left
        .width
        .checked_add(1)
        .and_then(|value| value.checked_add(right.width))
        .ok_or("The pair image width overflowed")?;
    let height = left.height.max(right.height);
    let mut pair = solid_image(width, height, PAIR_BACKGROUND);
    alpha_blit(&mut pair, &left, 0, (height - left.height) / 2)?;
    for y in 0..height {
        set_pixel(&mut pair, left.width, y, PAIR_SEPARATOR);
    }
    alpha_blit(
        &mut pair,
        &right,
        left.width + 1,
        (height - right.height) / 2,
    )?;
    fs::write(
        output_dir.join(format!("{case_id}-pair.png")),
        encode_png(&pair)?,
    )?;
    Ok(())
}

fn stack_images(images: &[DecodedPng], gap: u32) -> Result<DecodedPng, Box<dyn Error>> {
    let width = images
        .iter()
        .map(|image| image.width)
        .max()
        .ok_or("No images were supplied for stacking")?;
    let gaps = gap
        .checked_mul(images.len().saturating_sub(1) as u32)
        .ok_or("The formula gap height overflowed")?;
    let height = images.iter().try_fold(gaps, |total, image| {
        total
            .checked_add(image.height)
            .ok_or("The stacked image height overflowed")
    })?;
    let mut output = solid_image(width, height, [0, 0, 0, 0]);
    let mut y = 0;
    for image in images {
        let x = (width - image.width) / 2;
        alpha_blit(&mut output, image, x, y)?;
        y += image.height + gap;
    }
    Ok(output)
}

fn error_placeholder() -> DecodedPng {
    let mut image = solid_image(
        ERROR_IMAGE_WIDTH,
        ERROR_IMAGE_HEIGHT,
        [0x36, 0x0d, 0x13, 0xff],
    );
    for y in 0..ERROR_IMAGE_HEIGHT {
        let x1 = y * ERROR_IMAGE_WIDTH / ERROR_IMAGE_HEIGHT;
        let x2 = ERROR_IMAGE_WIDTH - 1 - x1;
        for offset in 0..3 {
            if x1 + offset < ERROR_IMAGE_WIDTH {
                set_pixel(&mut image, x1 + offset, y, [0xf8, 0x51, 0x49, 0xff]);
            }
            if x2 >= offset {
                set_pixel(&mut image, x2 - offset, y, [0xf8, 0x51, 0x49, 0xff]);
            }
        }
    }
    image
}

fn solid_image(width: u32, height: u32, color: [u8; 4]) -> DecodedPng {
    let mut rgba = vec![0; width as usize * height as usize * 4];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(&color);
    }
    DecodedPng {
        width,
        height,
        rgba,
    }
}

fn alpha_blit(
    destination: &mut DecodedPng,
    source: &DecodedPng,
    offset_x: u32,
    offset_y: u32,
) -> Result<(), Box<dyn Error>> {
    if offset_x + source.width > destination.width || offset_y + source.height > destination.height
    {
        return Err("The image blit exceeds the destination bounds".into());
    }
    for y in 0..source.height {
        for x in 0..source.width {
            let source_pixel = get_pixel(source, x, y);
            let destination_pixel = get_pixel(destination, offset_x + x, offset_y + y);
            set_pixel(
                destination,
                offset_x + x,
                offset_y + y,
                blend(source_pixel, destination_pixel),
            );
        }
    }
    Ok(())
}

fn blend(source: [u8; 4], destination: [u8; 4]) -> [u8; 4] {
    // Source-over compositing that preserves destination transparency, so
    // stacked native output stays transparent outside the glyphs.
    let source_alpha = source[3] as u32;
    let destination_alpha = destination[3] as u32;
    let inverse = 255 - source_alpha;
    let out_alpha = source_alpha + (destination_alpha * inverse + 127) / 255;
    if out_alpha == 0 {
        return [0, 0, 0, 0];
    }
    let channel = |s: u8, d: u8| -> u8 {
        let premultiplied =
            s as u32 * source_alpha + (d as u32 * destination_alpha * inverse + 127) / 255;
        ((premultiplied + out_alpha / 2) / out_alpha) as u8
    };
    [
        channel(source[0], destination[0]),
        channel(source[1], destination[1]),
        channel(source[2], destination[2]),
        out_alpha as u8,
    ]
}

fn get_pixel(image: &DecodedPng, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * image.width + x) * 4) as usize;
    [
        image.rgba[index],
        image.rgba[index + 1],
        image.rgba[index + 2],
        image.rgba[index + 3],
    ]
}

fn set_pixel(image: &mut DecodedPng, x: u32, y: u32, value: [u8; 4]) {
    let index = ((y * image.width + x) * 4) as usize;
    image.rgba[index..index + 4].copy_from_slice(&value);
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

fn encode_png(image: &DecodedPng) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, image.width, image.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&image.rgba)?;
    }
    Ok(bytes)
}

fn decode_base64(value: &str) -> Result<Vec<u8>, String> {
    let compact = value
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    if compact.len() % 4 != 0 {
        return Err("base64_invalid_length".to_string());
    }
    let mut output = Vec::with_capacity(compact.len() / 4 * 3);
    for (index, chunk) in compact.chunks_exact(4).enumerate() {
        let last = index + 1 == compact.len() / 4;
        let a = base64_value(chunk[0])?;
        let b = base64_value(chunk[1])?;
        let c = if chunk[2] == b'=' {
            if !last || chunk[3] != b'=' {
                return Err("base64_invalid_padding".to_string());
            }
            None
        } else {
            Some(base64_value(chunk[2])?)
        };
        let d = if chunk[3] == b'=' {
            if !last {
                return Err("base64_invalid_padding".to_string());
            }
            None
        } else {
            Some(base64_value(chunk[3])?)
        };
        if c.is_none() && d.is_some() {
            return Err("base64_invalid_padding".to_string());
        }
        output.push((a << 2) | (b >> 4));
        if let Some(c) = c {
            output.push((b << 4) | (c >> 2));
            if let Some(d) = d {
                output.push((c << 6) | d);
            }
        }
    }
    Ok(output)
}

fn base64_value(byte: u8) -> Result<u8, String> {
    match byte {
        b'A'..=b'Z' => Ok(byte - b'A'),
        b'a'..=b'z' => Ok(byte - b'a' + 26),
        b'0'..=b'9' => Ok(byte - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err("base64_invalid_character".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::decode_base64;

    #[test]
    fn base64_decoder_handles_padding() {
        assert_eq!(decode_base64("TQ==").expect("one byte"), b"M");
        assert_eq!(decode_base64("TWE=").expect("two bytes"), b"Ma");
        assert_eq!(decode_base64("TWFu").expect("three bytes"), b"Man");
    }
}
