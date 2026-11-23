//! `cargo run -p rf-win --example psd_thumb -- <out dir> <size> <file.psd>…`: each file's picture as PNG, with the time.
fn main() {
    let mut args = std::env::args().skip(1);
    let out = std::path::PathBuf::from(args.next().expect("out dir"));
    let size: u32 = args.next().expect("size").parse().unwrap();
    std::fs::create_dir_all(&out).unwrap();
    for (i, f) in args.enumerate() {
        let p = std::path::Path::new(&f);
        let t = std::time::Instant::now();
        let r = rf_win::psd_thumb::thumbnail(p, size);
        let ms = t.elapsed().as_millis();
        let mb = std::fs::metadata(p).map(|m| m.len() >> 20).unwrap_or(0);
        match r {
            Some(b) => {
                let mut d = b.data;
                for px in d.chunks_exact_mut(4) {
                    px.swap(0, 2);
                }
                image::RgbaImage::from_raw(b.width, b.height, d).unwrap().save(out.join(format!("{i:02}.png"))).unwrap();
                println!("{i:02} ok   {:>4}x{:<4} {ms:>5} ms {mb:>5} MB  {f}", b.width, b.height);
            }
            None => println!("{i:02} NONE           {ms:>5} ms {mb:>5} MB  {f}"),
        }
    }
}
