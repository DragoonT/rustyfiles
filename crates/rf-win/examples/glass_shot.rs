//! Windowless picture harness for the glass: loads a PNG, draws the glass over it with `rf_win::glass::render_offscreen_frames`, pastes the
//! result into the picture at the rectangle and writes the whole picture as PNG. No window is created, nothing is shown.
//!
//!   cargo run -p rf-win --example glass_shot -- --src IN.png --out OUT.png --rect X,Y,W,H [--dark] [--scale F] [--radius PX]
//!       [--lens X,Y,W,H,R]... [--clip X,Y,W,H] [--frost N] [--tint N] [--refraction N] [--saturation N] [--no-calm]
//!       [--shape X,Y,W,H] [--shadow]
//!   cargo run -p rf-win --example glass_shot -- --src IN.png --out SHEET.png --rect X,Y,W,H [--dark] --slide X0,Y0,X1,Y1,W,H,R
//!       [--frames N] [--hz F] [--crop W,H]
//!
//! `--scale` is device px per CSS px (default 1), `--radius` the corner radius in device px (default 8 x scale, like a window).
//! `--lens` puts a lens (a selection pill) on the glass: x, y, w, h, radius in device px relative to the glass; it can be given up to eight
//! times. `--clip` cuts all the lenses to a rectangle (default: the whole glass).
//! `--frost`, `--tint`, `--refraction` and `--saturation` are the four values of the HyperGlass settings, each 0 to 100 (defaults 52, 50,
//! 67, 62: the shipped look). `--no-calm` draws a small piece with the rim of a window, as it was before the rim of small pieces was bounded.
//! `--shape X,Y,W,H` makes `--rect` a larger transparent WINDOW and the glass only this rectangle of it (device px from the window's top
//! left corner), as the dock's islands are drawn live; `--radius` is then the shape's. `--shadow` adds the dock's soft shadow outside the
//! shape (black 26 % at the outline, 4 px down, gone 22 px away). The picture shows the window's pixels over the source; where the
//! window is transparent the source shows.
//! `--slide` simulates a lens that slides from (X0, Y0) to (X1, Y1), W x H with radius R, with the engine's own springs at `--hz` refreshes
//! per second (default 120), picks `--frames` of the simulated refreshes spread evenly over the whole motion (default 12) and writes ONE
//! contact sheet to `--out`: a crop of `--crop W,H` (default 260,240) around the lens for every frame, four per row. The time of each
//! frame is printed. Nothing is shown on any screen.

use std::error::Error;
use std::fs::File;
use std::io::{BufReader, BufWriter};

use rf_win::glass::{LensShot, LookParams};

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

/// Parses `a,b,c...` into exactly N numbers.
fn floats<const N: usize>(s: &str) -> Result<[f32; N], Box<dyn Error>> {
    let v: Vec<f32> = s.split(',').map(|x| x.trim().parse()).collect::<Result<_, _>>()?;
    v.try_into().map_err(|v: Vec<f32>| format!("{s}: needs {N} numbers, has {}", v.len()).into())
}

/// Pastes the glass: it is premultiplied, the picture shows through where it is not opaque.
fn paste(pic: &mut [u8], w: u32, h: u32, glass: &[u8], rect: (i32, i32, u32, u32)) {
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
}

struct Args {
    src: String,
    out: String,
    rect: (i32, i32, u32, u32),
    dark: bool,
    scale: f32,
    radius: Option<f32>,
    lenses: Vec<[f32; 5]>,
    params: LookParams,
    calm: bool,
    clip: Option<[f32; 4]>,
    shape: Option<[f32; 4]>,
    shadow: bool,
    slide: Option<[f32; 7]>,
    frames: usize,
    hz: f32,
    crop: (u32, u32),
}

fn parse() -> Result<Args, Box<dyn Error>> {
    let (mut src, mut out, mut rect, mut dark, mut scale, mut radius) = (None, None, None, false, 1.0f32, None::<f32>);
    let (mut lenses, mut clip, mut slide, mut frames, mut hz, mut crop) = (Vec::new(), None, None, 12usize, 120.0f32, (260u32, 240u32));
    let (mut shape, mut shadow) = (None, false);
    let (mut params, mut calm) = (LookParams::DEFAULT, true);
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
            "--lens" => lenses.push(floats(&it.next().ok_or("--lens needs X,Y,W,H,R")?)?),
            "--frost" => params.frost = it.next().ok_or("--frost needs 0 to 100")?.parse()?,
            "--tint" => params.tint = it.next().ok_or("--tint needs 0 to 100")?.parse()?,
            "--refraction" => params.refraction = it.next().ok_or("--refraction needs 0 to 100")?.parse()?,
            "--saturation" => params.saturation = it.next().ok_or("--saturation needs 0 to 100")?.parse()?,
            "--no-calm" => calm = false,
            "--shape" => shape = Some(floats(&it.next().ok_or("--shape needs X,Y,W,H")?)?),
            "--shadow" => shadow = true,
            "--clip" => clip = Some(floats(&it.next().ok_or("--clip needs X,Y,W,H")?)?),
            "--slide" => slide = Some(floats(&it.next().ok_or("--slide needs X0,Y0,X1,Y1,W,H,R")?)?),
            "--frames" => frames = it.next().ok_or("--frames needs a number")?.parse()?,
            "--hz" => hz = it.next().ok_or("--hz needs a number")?.parse()?,
            "--crop" => {
                let v: Vec<u32> = it.next().ok_or("--crop needs W,H")?.split(',').map(|s| s.trim().parse()).collect::<Result<_, _>>()?;
                if v.len() != 2 || v[0] == 0 || v[1] == 0 {
                    return Err("--crop needs W,H with a positive size".into());
                }
                crop = (v[0], v[1]);
            }
            o => return Err(format!("unknown argument {o}").into()),
        }
    }
    Ok(Args {
        src: src.ok_or("--src is required")?,
        out: out.ok_or("--out is required")?,
        rect: rect.ok_or("--rect is required")?,
        dark,
        scale,
        radius,
        lenses,
        params,
        calm,
        clip,
        shape,
        shadow,
        slide,
        frames,
        hz,
        crop,
    })
}

/// The contact sheet of a slide.
fn slide_sheet(a: &Args, w: u32, h: u32, pic: &[u8], sl: [f32; 7]) -> Result<(), Box<dyn Error>> {
    let t = std::time::Instant::now();
    let radius = a.radius.unwrap_or(8.0 * a.scale);
    let (from, to) = ([sl[0], sl[1], sl[4], sl[5]], [sl[2], sl[3], sl[4], sl[5]]);
    let sim = rf_win::glass::simulate_slide(from, to, a.hz);
    let n = a.frames.clamp(1, sim.len());
    // frame k of n: the simulated refresh nearest to k / (n - 1) of the way through the motion, the first and the last included
    let pick: Vec<usize> = (0..n).map(|k| if n == 1 { 0 } else { (k * (sim.len() - 1) + (n - 1) / 2) / (n - 1) }).collect();
    let shots: Vec<Vec<LensShot>> = pick.iter().map(|&i| vec![LensShot { rect: sim[i], clip: a.clip, radius: sl[6], opacity: 1.0 }]).collect();
    let glasses = rf_win::glass::render_offscreen_look((pic, w, h), a.rect, radius, a.scale, a.dark, &a.params, a.calm, &shots)?;
    println!(
        "{n} frames drawn in {:.0} ms; the slide takes {} refreshes at {} Hz = {:.0} ms",
        t.elapsed().as_secs_f64() * 1000.0,
        sim.len(),
        a.hz,
        (sim.len() - 1) as f64 * 1000.0 / a.hz as f64
    );
    // the crop is centred on the middle of everything the lens covers during the slide, in picture px
    let (ux0, uy0) = (from[0].min(to[0]), from[1].min(to[1]));
    let (ux1, uy1) = ((from[0] + from[2]).max(to[0] + to[2]), (from[1] + from[3]).max(to[1] + to[3]));
    let (cx, cy) = (a.rect.0 as f32 + (ux0 + ux1) / 2.0, a.rect.1 as f32 + (uy0 + uy1) / 2.0);
    let (cw, ch) = (a.crop.0.min(w), a.crop.1.min(h));
    let ox = ((cx - cw as f32 / 2.0).round() as i32).clamp(0, (w - cw) as i32) as u32;
    let oy = ((cy - ch as f32 / 2.0).round() as i32).clamp(0, (h - ch) as i32) as u32;
    let (cols, gap) = (4u32, 4u32);
    let rows = (n as u32).div_ceil(cols);
    let (sw, sh) = (cols * cw + (cols + 1) * gap, rows * ch + (rows + 1) * gap);
    let mut sheet = vec![24u8; (sw * sh * 4) as usize];
    for (k, glass) in glasses.iter().enumerate() {
        let mut frame = pic.to_vec();
        paste(&mut frame, w, h, glass, a.rect);
        let (tx, ty) = (gap + (k as u32 % cols) * (cw + gap), gap + (k as u32 / cols) * (ch + gap));
        for y in 0..ch {
            let si = (((oy + y) * w + ox) * 4) as usize;
            let di = (((ty + y) * sw + tx) * 4) as usize;
            sheet[di..di + (cw * 4) as usize].copy_from_slice(&frame[si..si + (cw * 4) as usize]);
        }
        let r = sim[pick[k]];
        println!("frame {k:2}: t = {:4.0} ms  lens y {:6.1}  h {:5.1}", pick[k] as f64 * 1000.0 / a.hz as f64, r[1], r[3]);
    }
    save(&a.out, sw, sh, &sheet)?;
    println!("wrote {} (crop {cw}x{ch} at {ox},{oy})", a.out);
    Ok(())
}

fn run() -> Result<(), Box<dyn Error>> {
    let a = parse()?;
    let (w, h, mut pic) = load(&a.src)?;
    if let Some(sl) = a.slide {
        return slide_sheet(&a, w, h, &pic, sl);
    }
    let radius = a.radius.unwrap_or(8.0 * a.scale);
    let t = std::time::Instant::now();
    if let Some(shape) = a.shape {
        let opts = rf_win::glass::GlassOptions {
            dark: a.dark,
            radius_px: Some(radius),
            cut_corners: true,
            shape: Some(shape),
            shadow: a.shadow.then_some(rf_win::glass::Shadow::SOFT),
        };
        let glass = rf_win::glass::render_offscreen_shape((&pic, w, h), a.rect, opts, a.scale, &a.params, a.calm)?;
        println!("shape {shape:?} in a window of {:?} drawn in {:.0} ms ({}{})", a.rect, t.elapsed().as_secs_f64() * 1000.0, if a.dark { "dark" } else { "light" }, if a.shadow { ", with the shadow" } else { "" });
        paste(&mut pic, w, h, &glass, a.rect);
        save(&a.out, w, h, &pic)?;
        println!("wrote {}", a.out);
        return Ok(());
    }
    let shots: Vec<LensShot> = a.lenses.iter().map(|l| LensShot { rect: [l[0], l[1], l[2], l[3]], clip: a.clip, radius: l[4], opacity: 1.0 }).collect();
    let glass = rf_win::glass::render_offscreen_look((&pic, w, h), a.rect, radius, a.scale, a.dark, &a.params, a.calm, &[shots])?.remove(0);
    println!(
        "glass drawn in {:.0} ms (rect {:?}, radius {radius} px, scale {}, {}, {} lenses, frost {} tint {} refraction {} saturation {}{})",
        t.elapsed().as_secs_f64() * 1000.0,
        a.rect,
        a.scale,
        if a.dark { "dark" } else { "light" },
        a.lenses.len(),
        a.params.frost,
        a.params.tint,
        a.params.refraction,
        a.params.saturation,
        if a.calm { "" } else { ", small pieces without the bounded rim" }
    );
    paste(&mut pic, w, h, &glass, a.rect);
    save(&a.out, w, h, &pic)?;
    println!("wrote {}", a.out);
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("glass_shot: {e}");
        std::process::exit(1);
    }
}
