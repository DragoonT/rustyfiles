//! Windowless picture harness for the glass: loads a PNG, draws the glass over it with `rf_win::glass::render_offscreen`, pastes the
//! result into the picture at the rectangle and writes the whole picture as PNG. No window is created, nothing is shown.
//!
//!   cargo run -p rf-win --example glass_shot -- --src IN.png --out OUT.png --rect X,Y,W,H [--dark] [--scale F] [--radius PX]
//!
//! `--scale` is device px per CSS px (default 1), `--radius` the corner radius in device px (default 8 x scale, like a window).

use std::error::Error;
use std::fs::File;
use std::io::{BufReader, BufWriter};

fn load(path: &str) -> Result<(u32, u32, Vec<u8>), Box<dyn Error>> {
    let mut dec = png::Decoder::new(BufReader::new(File::open(path)?));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut rd = dec.read_info()?;
    let mut buf = vec![0u8; rd.output_buffer_size().ok_or("png too large")?];
    let info = rd.next_frame(&mut buf)?;
    let n = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        c => return Err(format!("png colour type {c:?} not handled").into()),
    };
    let (w, h) = (info.width, info.height);
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for p in buf[..(w * h) as usize * n].chunks_exact(n) {
        let (r, g, b) = if n >= 3 { (p[0], p[1], p[2]) } else { (p[0], p[0], p[0]) };
        bgra.extend_from_slice(&[b, g, r, 255]);
    }
    Ok((w, h, bgra))
}

fn save(path: &str, w: u32, h: u32, bgra: &[u8]) -> Result<(), Box<dyn Error>> {
    if let Some(dir) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for p in bgra.as_chunks::<4>().0 {
        rgb.extend_from_slice(&[p[2], p[1], p[0]]);
    }
    let mut enc = png::Encoder::new(BufWriter::new(File::create(path)?), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgb)?;
    Ok(())
}

fn run() -> Result<(), Box<dyn Error>> {
    let (mut src, mut out, mut rect, mut dark, mut scale, mut radius) = (None, None, None, false, 1.0f32, None::<f32>);
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--src" => src = it.next(),
            "--out" => out = it.next(),
            "--rect" => {
                let v: Vec<i32> = it.next().ok_or("--rect needs X,Y,W,H")?.split(',').map(|s| s.trim().parse()).collect::<Result<_, _>>()?;
                if v.len() != 4 || v[2] <= 0 || v[3] <= 0 {
                    return Err("--rect needs X,Y,W,H with a positive size".into());
                }
                rect = Some((v[0], v[1], v[2] as u32, v[3] as u32));
            }
            "--dark" => dark = true,
            "--scale" => scale = it.next().ok_or("--scale needs a number")?.parse()?,
            "--radius" => radius = Some(it.next().ok_or("--radius needs a number")?.parse()?),
            o => return Err(format!("unknown argument {o}").into()),
        }
    }
    let (src, out, rect) = (src.ok_or("--src is required")?, out.ok_or("--out is required")?, rect.ok_or("--rect is required")?);
    let (w, h, mut pic) = load(&src)?;
    let radius = radius.unwrap_or(8.0 * scale);
    let t = std::time::Instant::now();
    let glass = rf_win::glass::render_offscreen(&pic, w, h, rect, radius, scale, dark)?;
    println!("glass drawn in {:.0} ms (rect {:?}, radius {radius} px, scale {scale}, {})", t.elapsed().as_secs_f64() * 1000.0, rect, if dark { "dark" } else { "light" });
    // paste: the glass is premultiplied, the picture shows through where it is not opaque
    let (x0, y0, rw, rh) = rect;
    for y in 0..rh as i32 {
        for x in 0..rw as i32 {
            let (px, py) = (x0 + x, y0 + y);
            if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                continue;
            }
            let gi = ((y as u32 * rw + x as u32) * 4) as usize;
            let di = ((py as u32 * w + px as u32) * 4) as usize;
            let inv = 255 - glass[gi + 3] as u32;
            for k in 0..3 {
                pic[di + k] = ((glass[gi + k] as u32 * 255 + pic[di + k] as u32 * inv + 127) / 255).min(255) as u8;
            }
        }
    }
    save(&out, w, h, &pic)?;
    println!("wrote {out}");
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("glass_shot: {e}");
        std::process::exit(1);
    }
}
