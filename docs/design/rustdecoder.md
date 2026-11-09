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
   **Phase 3, H.264 done (2026-11-09):** progressive 4:2:0 8-bit Baseline, Main and High, CAVLC
   and CABAC, from MP4 / MOV (`avcC`). New: `h264` (SPS with VUI reorder / DPB limits and scaling
   lists, PPS, slice header up to dec_ref_pic_marking, `avcC` through `nal::avcc`), `h264_dpb`
   (POC types 0, 1 and 2, sliding window and MMCO 1 to 6, IDR / MMCO5 flush,
   `no_output_of_prior_pics`, bumping by the stream's own `max_num_reorder_frames` or else the
   level's DPB size), `dxva_h264` (`DXVA_PicParams_H264`, `DXVA_Qmatrix_H264`; the short slice
   structure is shared with HEVC), `h264dec` (`H264Decoder`, same shape as `HevcDecoder`), `video`
   (`VideoDecoder`: opens the file, picks the decoder from the track's codec, same methods).
   Findings: for H.264 `ConfigBitstreamRaw == 2` is the short slice format and 1 is the long one
   (the reverse of HEVC); sending short structures to the raw 1 configuration decodes nothing (a
   black surface, no error), so `HwDecoder::with_alignment` takes the value that means short
   and also requires the configuration without bitstream encryption. H.264 surfaces are rounded
   up to 16 (HEVC: 128). `DecoderBeginFrame` can answer `E_PENDING` when decoding runs far
   ahead of the GPU; it is retried like `DXGI_ERROR_WAS_STILL_DRAWING`. Unsupported, with
   a clear error: MBAFF (at open), field pictures (at the first one), FMO (PPS), data partitioning,
   4:2:2 / 4:4:4, more than 8 bits, lossless transform bypass, frame_num gaps. A stream with
   `frame_mbs_only_flag` 0 whose pictures are all frames decodes. Streams starting on a non-IDR
   intra picture start there (earlier pictures dropped).
   Tests (`tests/gpu_h264.rs`, x264 clips 640x360, 3 s unless noted, compared sample by sample
   with `ffmpeg -f rawvideo` in display order): baseline CAVLC (POC type 2), main CABAC with
   B-pyramid (MMCO 1), main CAVLC, high 8x8 transform, high with `cqm=jvt` (PPS scaling lists),
   4 slices, `weightp=2:weightb=1` on a fade (explicit and implicit weights), 8 reference
   frames, 9 s long GOP (270 frames), open GOP (non-IDR I pictures), strict B-pyramid with
   MMCO, `fake-interlaced` (frame_mbs_only 0), 1080p level 4.1 phone style, and NVENC High
   with B-pyramid: all bit-exact. MBAFF (`tff=1`) is refused. A 10 s 1080p60 High clip
   (600 frames, noisy) is bit-exact too and decodes at about 2000 fps (GPU only, no read-back,
   RTX 5070 Ti). MMCO 2 to 6, long-term pictures and POC type 1 have only unit tests in
   `h264_dpb` (x264 and NVENC never produce them).
   Left for H.264: interlaced (field / MBAFF / PAFF), frame_num gap concealment, High 10 and
   4:2:2 profiles, the long slice format for GPUs offering only that.
4. Playback in DirectLook: clock, frame queue, seeking, audio; replaces the Media Foundation
   route, and FilmCraft for video.
5. MKV / WebM, VP9 and AV1 (the GPU decodes them too), fragmented MP4.
6. Formats no GPU decodes, on the CPU: ProRes and DNxHR (intra-only, slices decode in
   parallel), MXF, AIFF. Editing sources must keep previewing.
7. FilmCraft removed in one step: dependency, `rf-player`'s FilmCraft code, texts and notes.
   Not before phases 4 and 6 cover what it plays today (decided 2026-11-09: no format may
   disappear in between).

Fallback: GPUs without a decoder for the stream (old cards, AV1 before 2020) keep the
Media Foundation route.
