# rustdecoder

rustyfiles' own video engine, replacing FilmCraft in DirectLook and the preview pane.

## Why

DirectLook decoded video with FilmCraft, on the CPU. A 4K 59.94 fps HEVC Main 10 clip
(Sony XAVC HS, 77 Mbit/s) decoded at 42 frames a second on a Ryzen 7 9800X3D with nothing
else running, and stuttered under load. Media Foundation (the GPU's video engine) plays it
smoothly, so DirectLook routes videos past 1080p60 there for now (commit 32e9c58).
Media Foundation is a black box, though: no frame-exact seeking, no control over presentation,
and the MFPlay API it goes through is deprecated.

No CPU decoder plays 4K60 10-bit HEVC smoothly on ordinary machines. Fast players (mpv, VLC,
Movies & TV) hand the compressed data to the graphics card's video engine. rustdecoder does
the same, with every step around that call written here:

| Step | Who |
| --- | --- |
| Reading the container (MP4 / MOV, later MKV / WebM) | rustdecoder |
| Splitting samples into NAL units, parsing VPS / SPS / PPS / slice headers | rustdecoder |
| Reference picture management (DPB, POC, RPS) | rustdecoder |
| Filling the DXVA picture parameters, slice controls, scaling lists | rustdecoder |
| Entropy decoding, inverse transform, prediction, filters | GPU video engine (D3D11 Video) |
| YUV → RGB, scaling, rotation, presentation (swap chain) | rustdecoder (shaders) |
| Timing, A/V sync, frame-exact seeking | rustdecoder |

Frames stay on the GPU from decode to screen: no copy back to system memory.

## Crate layout

`crates/rustdecoder`, no dependency on FilmCraft or rf-app.

- `mp4` — ISO BMFF reader. Box walk, `moov` → tracks: timescale, duration, rotation (`tkhd`
  matrix), sample description (`avcC`, `hvcC`, audio), sample table (`stts`, `ctts`, `stsc`,
  `stsz`/`stz2`, `stco`/`co64`, `stss`) flattened into one list: offset, size, decode time,
  presentation time, keyframe. Fragmented MP4 (`moof`) later.
- `bits` — bit reader with Exp-Golomb and emulation-prevention removal.
- `h264`, `hevc` — parameter sets and slice headers.
- `d3d11` — video device, decoder creation by profile (H.264, HEVC Main, HEVC Main 10),
  per-frame submission, output texture array.
- `present` — NV12 / P010 shaders, rotation, HDR → SDR tone mapping.
- `audio` — PCM and AAC via the OS decoder at first; own AAC later.

## Phases

1. MP4 reader with tests against ffprobe (sample count, sizes, timestamps, keyframes).
2. Bit reader, H.264 / HEVC parameter sets and slice headers.
3. D3D11 Video decode of HEVC Main / Main 10 and H.264 High, first frame to a PNG in a test.
   **Phase 3, HEVC done (2026-11-09):** `d3d11` (adapter pick by profile, decoder, texture
   array, submission), `slice` (slice header up to the long-term pictures; the GPU reads the
   rest), `dpb` (POC, RPS, marking, bumping, CRA/RASL skipping), `dxva` (PicParams, Qmatrix,
   short slice control), `decoder` (`HevcDecoder::open / next_frame / read_back`). Output is
   bit-exact with ffmpeg on Main and Main 10 clips with B-frames, open GOP pyramid, default
   scaling lists, 4 slices per picture, and on the first 60 frames of the Sony XAVC HS 4K60
   clip (7 slices, PPS scaling lists, leading RADL pictures). That clip decodes at 511 fps
   (330 frames, 0.65 s, GPU only, RTX 5070 Ti, short slice format, ConfigBitstreamRaw 1).
   Assumptions: short slice format only (a GPU offering only the long format is refused),
   4:2:0 8/10-bit, surfaces rounded up to 128, one frame stays valid until the next
   `next_frame`. Left for phase 4: frame queue holding several surfaces, shaders, seeking
   (start at a keyframe, drop RASL), mid-stream SPS changes beyond size, other GPUs.
   H.264 is not started.
4. Playback in DirectLook: clock, frame queue, seeking, audio; replaces the Media Foundation
   route, and FilmCraft for video.
   **Phase 4 groundwork, headless core done (2026-11-09):**
   - `present` (`Converter`): NV12 / P010 slice → BGRA8 texture of any size on the decoder's
     device, one pixel shader compiled with `D3DCompile` when the converter is created (no
     shader tools in the build). Chroma upsampling is the sampler's bilinear filter on the half
     size chroma plane (centre siting), the matrix comes from the VUI (`Colorimetry::from_vui`:
     BT.709 / 601 / 2020 / SMPTE 240, unsignalled streams BT.709 when 1280 wide or more or over
     576 lines, else BT.601; limited or full range; 8 and 10 bit), rotation 0 / 90 / 180 / 270
     from the `tkhd` matrix, optional mirror, scaling by bilinear sampling of the visible
     rectangle (clamped half a sample inside it so the padding never bleeds in). HDR → SDR,
     **first version**: PQ and HLG are tone mapped with the BT.2390 EETF (1000 nit source peak
     assumed, no mastering display metadata read yet; target peak 203 nits = SDR white, black
     level 0, per channel), BT.2020 → BT.709 primaries, sRGB encoding. HLG goes through the
     1000 nit display OOTF (gamma 1.2) first. Per-channel mapping desaturates very bright
     colours a little, and 203 nit white lands at about 90 % (the curve's knee), by design of
     the first version. Timestamp queries give the GPU time per conversion.
   - `player` (`Player`): a worker thread owns the `HevcDecoder` and copies every output picture
     out of the decoder's array into a ring of 2..n slots (one texture array, same format);
     the ring bounds the decode-ahead (a slot is taken before decoding and returned when the
     picture is replaced). `Clock` (start / pause / resume / seek / time) over a `TimeSource`
     (`SystemTime`, `FakeTime` for tests). `frame_for(now)` shows the latest picture whose time
     has come and never one from the future; passed-over pictures count as dropped, pictures
     shown more than a frame late as late. `seek(t)` plans from the sample table
     (`plan_seek`): the picture with the largest pts not after `t` is the landing picture; decoding
     restarts at the sync sample before it, one sync sample further back so the RASL pictures
     of a CRA picture decode, and everything before the landing picture is dropped without a
     copy. Stats: decoded, skipped by seek, dropped, late, shown, most slots in use.
     D3D11's multithread protection is switched on for the device because the decoder thread
     (decode, copy) and the caller (convert) share the immediate context.
   - `HevcDecoder` got `gpu()`, `track()`, `seek_to_sample()`; `StreamInfo` got `rotation` and
     `vui`.
   - Checked: colour against ffmpeg (mean abs error per channel 0.04 to 0.5 of 255, see tests);
     rotation against ffmpeg's autorotate; PQ / HLG grey ramps against the curve worked out
     separately; queue order and repeats with a fake clock; ring bound; 31 seeks on an open GOP
     clip landing on the right picture with the same pixels as a linear decode. Camera clip
     (4K 59.94 fps Main 10, stored rotated 270): 5 s at real-time speed, decode + convert to
     1080x1920, 298..300 of 300 frames shown, 0..2 dropped, 0 late, 0.5 ms GPU per conversion
     (`examples/play_headless.rs`).
   - Left for phase 4: window / swap chain presentation in DirectLook (flip model swap chain,
     present the converted target or convert straight into the back buffer, vsync-aligned
     `frame_for`), audio (decode, output device, the audio clock as the master clock and A/V
     sync by dropping or repeating pictures against it), pause on a seek target, playback
     rate, looping, mid-stream size changes (the ring is built from the first picture), mastering
     display / content light metadata for the tone map, chroma siting other than centre,
     a better downscale filter than bilinear for large ratios, edit lists.
5. MKV / WebM, VP9 and AV1 (the GPU decodes them too), fragmented MP4.
6. Formats no GPU decodes, on the CPU: ProRes and DNxHR (intra-only, slices decode in
   parallel), MXF, AIFF. Editing sources must keep previewing.
7. FilmCraft removed in one step: dependency, `rf-player`'s FilmCraft code, texts and notes.
   Not before phases 4 and 6 cover what it plays today (decided 2026-11-09: no format may
   disappear in between).

Fallback: GPUs without a decoder for the stream (old cards, AV1 before 2020) keep the
Media Foundation route.
