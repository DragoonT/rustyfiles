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
| Reading the container (MP4 / MOV, MKV / WebM) | rustdecoder |
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
- `mkv` — Matroska / WebM reader producing the same `Movie` / `Track` / flat sample table as
  `mp4` (see phase 5); `container::read` picks the reader from the first bytes.
- `bits` — bit reader with Exp-Golomb and emulation-prevention removal.
- `h264`, `hevc` — parameter sets and slice headers.
- `vp9`, `dxva_vp9`, `vp9dec` — VP9 superframe split, uncompressed header, `DXVA_PicParams_VP9`,
  `Vp9Decoder` (phase 5).
- `d3d11` — video device, decoder creation by profile (H.264, HEVC Main, HEVC Main 10, VP9 0 and 2),
  per-frame submission, output texture array.
- `present` — NV12 / P010 shaders, rotation, HDR → SDR tone mapping.
- `audio`, `resample`, `audio_out`, `playback` — PCM and AAC (OS decoder at first, own AAC later), sample-rate conversion, WASAPI output with the audio clock, A/V sync.

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
   **Phase 4, audio and A/V sync done (2026-11-09):**
   - `audio` (`AudioDecoder`): first audio track of an MP4 / MOV as interleaved `f32` blocks with
     their time in seconds. PCM (`lpcm`, `sowt`, `twos`, `in24`, `in32`, `fl32`, `fl64`, `ipcm`,
     `fpcm`; 8 / 16 / 24 / 32 bit int, 32 / 64 bit float, both byte orders) is converted here;
     AAC goes through Windows' `CMSAACDecMFT` (raw frames in, AudioSpecificConfig as
     `MF_MT_USER_DATA` after a 12 byte HEAACWAVEINFO tail, `MF_MT_AAC_PAYLOAD_TYPE` 0, float
     out). No FilmCraft, no ffmpeg at runtime. The decoder drops every frame before time 0 and
     before a seek target; the AAC encoder delay is the first edit of `elst`, which `mp4.rs`
     already applied (first packet at -1024), so the first sample returned is the sample at 0.
     A seek restarts AAC two packets before the target (priming) and trims.
   - `resample` (`Resampler`, `map_channels`): streaming windowed-sinc (Blackman-Harris, 16 taps
     per side, kernel stretched and cut off at 0.97 of the new Nyquist when going down, table of
     512 steps with linear interpolation), no latency to compensate. Channels: mono to the front
     pair, stereo to mono, 5.1 / 7.1 to stereo by ITU weights normalised so full scale cannot
     clip, others by common channels.
   - `audio_out` (`AudioOutput`, trait `AudioSink`): WASAPI shared mode, event driven, device mix
     format (float32; 16-bit PCM mix formats also handled), a render thread filling from a queue.
     The clock is `IAudioClock::GetPosition` interpolated with its QPC time (at most 20 ms,
     never backwards), with the silence written for under-runs taken out once played, so the
     clock stands still while the sound does; after `end_of_data` silence is time. It is a
     `TimeSource` (`AudioClockSource`): seconds of sound played, still when paused. Muting is
     explicit (`OutputOptions::muted`, the default): session volume 0, session mute, and silent
     buffers, all before the stream starts.
   - `playback` (`Playback`): `Player` plus an audio feed thread. The player's `Clock` runs over
     the sink's clock, so the picture shown is `frame_for` of the sound heard; the late / drop
     policy is the player's. `play` waits for 150 ms of queued sound before starting the device,
     `seek(t)` moves the video as before and the audio to the packet containing `t` with the
     sound before `t` dropped (the sink is flushed by the feed thread, so nothing old is
     queued after the call returns), pause / play keep both in step. No audio track, no device
     or an unsupported audio codec leave the system clock; `audio_note()` says why.
   - Checked (all on generated clips with ffmpeg as the reference, tests `audio`, `playback_av`,
     lib tests): PCM s16le / s24le in MOV and mono 44.1 kHz bit-exact with `ffmpeg -f f32le`
     (max diff 0, same length); AAC-LC stereo 48 kHz max abs diff 1.1e-3, RMS 2.5e-5, AAC mono
     44.1 kHz 1.5e-4 / 3.2e-6 (full scale 1.0), the first sample at 0.000 s and the length 144384
     / 133120 frames, identical to ffmpeg's (edit list handled); seeks land on the exact frame
     (PCM identical to a linear decode, AAC the same except the last 100 frames of the stream,
     where a second pass of the OS decoder rounds a little differently). Resampler: a 1 kHz sine
     keeps 1000.000 Hz and its level within 0.0002 dB for 44.1 to 48, 48 to 44.1, 96 to 48 and
     22.05 to 48 kHz; 30 kHz at 96 kHz to 48 kHz comes out below -60 dB. Sync with a fake audio
     clock: pictures in order and never from the future, none dropped at 60 Hz, audio blocks one
     unbroken run from 0, after seeks to 1.5 / 0.2 / 2.9 / 0 s the first audio block starts at
     the target (within 1/48000) and the first picture is the one at or just before it. Muted
     real device: clock advances 0.4929 s in 0.5032 s of wall time, 0 under-runs, 51 periods per
     half second; an under-run stops the clock (0.3 s queued, 0.9 s wall: 0.300 s).
   - Camera clip (Sony XAVC HS, `20261011_C5743.MP4`): audio is PCM `twos` (16-bit big endian)
     48 kHz stereo, 264264 frames; the device mixes at 48 kHz so nothing is resampled. 5 s muted,
     4K59.94 HEVC Main 10 converted to 1080x1920: clock advanced 4.991 s, 300 of 300 pictures
     shown, 0 dropped, 0 late, 0 audio under-runs, picture minus sound heard between -7.9 and
     0 ms (never more than half a frame; trend +0.5 ms per second, no creeping), longest wait
     between two pictures 25 ms.
   - Left for audio: HE-AAC / AAC-PS only written for, not tested (no encoder here), AC-3 / E-AC-3
     / Opus / FLAC, several audio tracks and track choice, 5.1 AAC channel order checked only
     by the downmix maths, `Player` is HEVC only (H.264 plays through `VideoDecoder` but not
     yet through `Player` / `Playback`), device change while playing (default device switched,
     `AUDCLNT_E_DEVICE_INVALIDATED`), exclusive mode / low latency, volume ramp on seek (a click
     is possible at a seek), playback rate (needs time stretching), edit lists beyond the first
     edit, AAC tail trim to the edit end, the system mixer's session volume persists per
     executable (muted runs of the test executables leave them muted).
5. MKV / WebM, VP9 and AV1 (the GPU decodes them too), fragmented MP4.
   **Phase 5, MKV / WebM and VP9 done (2026-11-09); AV1 and fragmented MP4 left:**
   - `mkv` (`mkv::read`, `container::read`): EBML walk of the Segment: Info (`TimestampScale`,
     `Duration`), Tracks (`CodecID` `V_MPEG4/ISO/AVC`, `V_MPEGH/ISO/HEVC`, `V_VP9`, `V_AV1`,
     `A_AAC*`, `A_OPUS`, `A_VORBIS`, `A_PCM/INT/LIT|BIG`, `A_PCM/FLOAT/IEEE`; `CodecPrivate`,
     pixel size, `Colour`, `FlagDefault`, `DefaultDuration`, `CodecDelay`, audio rate / channels /
     bit depth), Clusters with SimpleBlock and BlockGroup (a group without `ReferenceBlock` is a
     sync sample), Xiph / EBML / fixed lacing (each laced frame is its own sample, timed by
     `DefaultDuration`, else `BlockDuration / n`), unknown-size Segment and Clusters (a cluster
     ends at the first element that cannot sit inside one), cut files (keeps what is complete).
     The result is the MP4 reader's `Movie`: `Track.samples` with offset, size, dts, pts, sync flag,
     so `HevcDecoder`, `H264Decoder`, `AudioDecoder`, `Player` and `Playback` read MKV through
     `container::read` unchanged. Only element headers and the bytes in front of each block are
     read (32 KiB window), one 32 byte `Sample` per frame stays in memory; payloads are never
     loaded. The `Cues` are skipped: every cluster is indexed, so the table already has every sync
     sample. A track's timescale is 1e9 / `TimestampScale` (1000), `pts` is the container's own
     integer (ffprobe prints the same). Audio times have `CodecDelay` taken off like ffmpeg does
     (Opus pre-skip, ffmpeg's AAC priming), so the first AAC packet starts before 0 as in MP4 and
     `AudioDecoder` trims it (it uses the exact delay for the first packet, not the rounded
     millisecond). PCM blocks follow each other: a start time rounded to the container's tick
     does not open gaps. Video `dts` are the `pts` sorted (Matroska has no decode time).
     Not handled, listed without samples: `ContentEncodings` (header stripping, compression,
     encryption: the frames are not a plain offset plus size). Video rotation (`Projection`) is not
     read yet. 8-bit PCM is read as signed like QuickTime's `twos`; Matroska's is unsigned.
   - Checked against ffprobe (`-show_packets`: count, size, pts, keyframe flag, all equal, every
     packet): H.264 + AAC mkv (90 + 142 packets), HEVC + PCM mkv (90 + 141), VP9 + Opus webm
     (90 + 151), VP9 + Vorbis webm (90 + 143), `-cluster_size_limit 30000 -cluster_time_limit 300
     -reserve_index_space 4000` (14 clusters), `-live 1` (unknown-size Segment, no Cues) for mkv
     and webm; `Colour` (BT.2020 / PQ / limited) read back. NAL lengths of every 7th H.264 / HEVC
     sample add up to the sample, VP9 frame markers of all samples are right (the offsets point at
     the frames). ffmpeg's muxer never laces and never writes unknown-size clusters, so a hand
     built file covers Xiph, EBML and fixed lacing (3 + 4 + 5 frames), block groups with and
     without references, an unknown-size Cluster followed by a sized one, Void elements and a
     cut file; ffprobe reads the hand built file with the same audio packets (pts, size).
   - `vp9` + `dxva_vp9` + `vp9dec` (`Vp9Decoder`, `VideoDecoder::Vp9`): `split_superframe`
     (the index at the end of a packet; hidden frames travel with the next shown one), the
     uncompressed header (frame type, `show_existing_frame`, `intra_only`, reset / refresh flags,
     reference indices and sign bias, size from a reference, interpolation filter, loop filter and
     its delta persistence, quantizer, segmentation with its persistence, tiles, compressed
     header size, colour config: profiles 0 to 3, 8 / 10 / 12 bit parsed), `Parser` keeps what a
     header depends on (deltas, features, colour, size of each of the 8 slots). `DXVA_PicParams_VP9`
     (208 bytes packed, laid out from the SDK's `dxva.h`; `frame_refs` and `ref_frame_map` hold
     surface indices like ffmpeg's `dxva2_vp9.c`; `use_prev_in_find_mv_refs` is: not error
     resilient, same size as the last decoded frame, and that frame was shown),
     `DXVA_Slice_VPx_Short` (one per frame, the whole frame is the bitstream: the video engine reads
     the compressed header and the tiles), no scaling lists. Short slice format is
     `ConfigBitstreamRaw` 1 as for HEVC. The host keeps 8 slots and a use count per surface (a pool
     of 12, surfaces 64 aligned); frames go out in decode order (VP9 does not reorder);
     `show_existing_frame` queues the slot's surface again. Profile 0 (NV12) and profile 2, 10 bit
     (P010). Refused with a clear error: profiles 1 and 3 (4:4:4 / 4:2:2 / 4:4:0), 12 bit, a bit
     depth change inside the stream, an inter frame larger than the surfaces made for the first
     key frame. A key frame of another size is fine: smaller keeps the surfaces, larger makes a
     new decoder (frames carry their own size).
   - Bit-exact against ffmpeg (libavcodec's VP9 decoder, `-f rawvideo`, every sample of every
     shown frame, `-noautoscale -fps_mode passthrough`), libvpx-vp9 clips 640x360 unless noted,
     3 s, 90 frames: defaults; 1-pass with `-lag-in-frames 25 -auto-alt-ref 1` (libvpx makes no
     hidden frames 1-pass); no alt ref, GOP 30; 1280x720 with `-tile-columns 2`; `-tile-columns 1
     -tile-rows 2`; `-error-resilient 1 -frame-parallel 1`; `-aq-mode 1` and `-aq-mode 3`
     (segmentation on 90 / 89 frames); `-lossless 1`; 642x362 (odd size); `-sharpness 5
     -static-thresh 200`; 2-pass `-auto-alt-ref 1` (8 superframes with hidden frames); 2-pass
     `-auto-alt-ref 6 -enable-tpl 1` on a clean source (20 superframes, 42 hidden frames, 42
     `show_existing_frame`); profile 2 10-bit, 1-pass and 2-pass multi-layer alt ref (17
     superframes, 36 hidden, 36 show-existing); a 640x360 stream joined to a 320x180 one at a key
     frame and the other way round (120 frames, both bit-exact); a 1080p60 8 Mbit clip of 300
     frames. Also H.264 and HEVC in mkv (B-frames, 90 frames each). All identical.
     libvpx in this ffmpeg has no resize mode, so inter frames with references of another size
     (scaled references), and `intra_only` frames (libvpx makes none here) are not exercised;
     both are written to the spec and the DXVA structure carries the reference sizes.
   - Speed (RTX 5070 Ti, GPU only, no read-back): 1080p60 VP9 8 Mbit, 300 frames in 0.20 s,
     about 1500 fps (HEVC / H.264 1080p: 500 to 2000 fps, same order).
   - `Playback::open` works on MKV / WebM with H.264, HEVC or VP9: the player and the sync
     tests (pictures in order next to the audio clock, seeks landing both sides on the target
     within 1/48000 s) pass on mkv with H.264 + AAC, HEVC + PCM, VP9 + PCM and VP9 + AAC. With
     Opus or Vorbis the video plays on the system clock (36 pictures in 1.2 s) and `audio_note()`
     says "audio track not playable: not supported yet: Opus audio is not decoded yet (PCM and AAC
     are)". Matroska times are whole milliseconds, so a 30 fps picture can be 0.7 ms off its true
     time (worst lag 33.7 ms instead of 33.3 ms at 60 Hz).
   - Left for phase 5: AV1 (`Codec::Av1` is recognised, no decoder: `D3D11_DECODER_PROFILE_AV1_VLD_*`,
     OBU parsing, film grain), Opus and Vorbis decoding (own, like the rest), fragmented MP4
     (`moof`), `ContentEncodings` header stripping (mkvmerge's default for some codecs), Matroska
     `Projection` rotation and HDR metadata into the tone mapper, VP9 4:4:4 (profile 1 / 3), 12 bit,
     scaled references and `intra_only` tests with a stream that has them, several video tracks
     (the default-flagged track is chosen), the `probe` in rf-app's `rd_video.rs` (still MP4 / MOV /
     M4V only; route `.mkv` / `.webm` through `container::read` there).
6. Formats no GPU decodes, on the CPU: ProRes and DNxHR (intra-only, slices decode in
   parallel), MXF, AIFF. Editing sources must keep previewing.
7. FilmCraft removed in one step: dependency, `rf-player`'s FilmCraft code, texts and notes.
   Not before phases 4 and 6 cover what it plays today (decided 2026-11-09: no format may
   disappear in between).

Fallback: GPUs without a decoder for the stream (old cards, AV1 before 2020) keep the
Media Foundation route.
