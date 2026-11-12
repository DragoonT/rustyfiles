//! The glass renderer: device, shaders, the constant buffer, the region pyramid, and the offscreen path. Everything here is
//! plain D3D11 on a device the caller owns; no window, no swap chain, no DirectComposition (those live in `engine.rs`).
//!
//! The live path and `render_offscreen` call the same functions (`prep`, `glass_pass`), so a picture made offscreen is what a window gets.

use std::ffi::c_void;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_IEEE_STRICTNESS, D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
    D3D_FEATURE_LEVEL_11_1, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D_SRV_DIMENSION_TEXTURE2D, ID3DBlob,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R10G10B10A2_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIAdapter;
use windows::core::{Interface, PCSTR};

type R<T> = windows::core::Result<T>;

pub(super) const SHADER_SRC: &str = include_str!("glass.hlsl");

/// Levels of the blur pyramid (0 = sharp), as in the experiment.
const MAX_LEVELS: u32 = 9;
/// The region is allocated in steps of this many pixels, so a live resize does not reallocate at every step.
const REGION_STEP: u32 = 256;
/// The loop guard reads back the first pyramid level at most this wide.
const GUARD_WIDTH: u32 = 160;

// ---------------------------------------------------------------------------------------------------------------------------------
// The look
// ---------------------------------------------------------------------------------------------------------------------------------

/// The numbers of the "Regular" preset of the optical model (see the header of glass.hlsl). Lengths are CSS px (96 dpi).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Look {
    pub depth: f32,
    pub bezel: f32,
    pub max_bezel: f32,
    pub zoom: f32,
    pub dispersion: f32,
    /// Gaussian deviation of the frost, CSS px.
    pub blur: f32,
    pub saturate: f32,
    pub lum: f32,
    pub contrast: f32,
    pub tint: [f32; 4],
    pub rim: f32,
    pub rim_width: f32,
    pub shade: f32,
    /// Direction of the rim light in degrees; y points down, so -135 is the top left.
    pub light: f32,
}

impl Look {
    /// The one look the app ships. No outer shadow (DWM draws the window's own), no zoom, no colour fringe.
    ///
    /// The shape, frost and saturation are the "Regular" preset of the model. The grade and the tint are not: the preset lowers the
    /// contrast to 0.81 (black becomes 24 of 255, white 229) and lays a 36 % dark or 50 % white tint over it, which is right for a
    /// small panel but turned a whole window into a flat grey slab (dark theme: 26 over a black desktop, 158 over a white page). So
    /// the dark glass keeps the contrast, dims what is behind it to 42 % and tints less (6 over black, 81 over white, colours stay
    /// colours); the light glass keeps the brightness and tints more towards white (70 %: 182 over a black desktop instead of 138).
    pub(super) fn regular(dark: bool) -> Look {
        Look {
            depth: 1.188,
            bezel: 0.421,
            max_bezel: 30.629,
            zoom: 1.0,
            dispersion: 0.0,
            blur: 10.351,
            saturate: 2.252,
            lum: if dark { 0.42 } else { 1.0 },
            contrast: if dark { 1.0 } else { 0.9 },
            tint: if dark { [20.0 / 255.0, 20.0 / 255.0, 24.0 / 255.0, 0.30] } else { [1.0, 1.0, 1.0, 0.70] },
            rim: 0.25,
            rim_width: 0.3,
            shade: 0.03,
            light: -135.0,
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------
// Constant buffer
// ---------------------------------------------------------------------------------------------------------------------------------

/// Constant buffer shared by all passes; mirrors `cbuffer CB` in glass.hlsl field by field (twelve float4).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct Cb {
    pub win: [f32; 4],
    pub reg: [f32; 4],
    pub shape: [f32; 4],
    pub a: [f32; 4],
    pub b: [f32; 4],
    pub c: [f32; 4],
    pub d: [f32; 4],
    pub tint: [f32; 4],
    pub pass: [f32; 4],
    pub ext: [f32; 4],
    pub sub: [f32; 4],
    pub opt: [f32; 4],
}

/// What one draw needs to know besides the look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Geometry {
    /// Size of the glass (the window's client area), device px.
    pub w: u32,
    pub h: u32,
    /// Where the glass's top left corner is in the source picture, source px.
    pub origin: (i32, i32),
    /// Device px per CSS px.
    pub scale: f32,
    /// Corner radius, device px (0 = square corners).
    pub radius_px: f32,
    /// Whether the glass cuts its own corners out (a free-standing shape). A window does not: the system cuts a window's corners to its
    /// own curve, and a second cut here that is not exactly the same leaves empty pixels between the two.
    pub cut_corners: bool,
}

/// Backdrop kept around the glass in the region, device px: the rays reach inward only, the blur needs a little more.
pub(super) fn region_pad(scale: f32) -> u32 {
    (96.0 * scale).round().max(1.0) as u32
}

fn make_cb(g: &Geometry, look: &Look, src: (u32, u32), region: &Region, check: bool) -> Cb {
    let s = g.scale;
    let pad = region.pad;
    Cb {
        win: [g.w as f32, g.h as f32, region.w as f32, region.h as f32],
        reg: [(g.origin.0 - pad as i32) as f32, (g.origin.1 - pad as i32) as f32, src.0 as f32, src.1 as f32],
        shape: [g.w as f32 / (2.0 * s), g.h as f32 / (2.0 * s), g.radius_px / s, s],
        a: [look.depth, look.bezel, look.max_bezel, look.zoom],
        b: [look.dispersion, look.blur, look.saturate, look.lum],
        c: [look.contrast, look.rim, look.rim_width, look.shade],
        // alpha 0 = no outer shadow
        d: [look.light, 0.0, 0.0, 0.0],
        tint: look.tint,
        pass: [0.0; 4],
        // x: non-finite check, y: debug 0, z: region pad, w: smooth sampling (never the point mode)
        ext: [check as u8 as f32, 0.0, pad as f32, 0.0],
        sub: [0.0; 4],
        opt: [(!g.cut_corners) as u8 as f32, 0.0, 0.0, 0.0],
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------
// Device and shaders
// ---------------------------------------------------------------------------------------------------------------------------------

pub(super) struct Gpu {
    pub dev: ID3D11Device,
    pub ctx: ID3D11DeviceContext,
}

impl Gpu {
    /// A device on one adapter (the one a monitor is attached to).
    pub(super) unsafe fn on_adapter(ad: &windows::Win32::Graphics::Dxgi::IDXGIAdapter1) -> R<Gpu> {
        let adapter: IDXGIAdapter = ad.cast()?;
        Gpu::create(Some(&adapter), D3D_DRIVER_TYPE_UNKNOWN)
    }

    /// Hardware if there is any, else WARP (a CI runner has no GPU). `force_warp` skips the hardware try.
    pub(super) unsafe fn offscreen(force_warp: bool) -> R<Gpu> {
        if !force_warp {
            match Gpu::create(None, D3D_DRIVER_TYPE_HARDWARE) {
                Ok(g) => return Ok(g),
                Err(e) => log::debug!("glass: no hardware device for the offscreen picture ({e}); using WARP"),
            }
        }
        Gpu::create(None, D3D_DRIVER_TYPE_WARP)
    }

    unsafe fn create(adapter: Option<&IDXGIAdapter>, kind: D3D_DRIVER_TYPE) -> R<Gpu> {
        let (mut dev, mut ctx) = (None, None);
        D3D11CreateDevice(
            adapter,
            kind,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut dev),
            None,
            Some(&mut ctx),
        )?;
        match (dev, ctx) {
            (Some(dev), Some(ctx)) => Ok(Gpu { dev, ctx }),
            _ => Err(fail()),
        }
    }
}

unsafe fn compile(entry: &str, target: &str, extra_flags: u32) -> Result<Vec<u8>, String> {
    let (mut blob, mut err): (Option<ID3DBlob>, Option<ID3DBlob>) = (None, None);
    let (n, e, t) = ("glass.hlsl\0", format!("{entry}\0"), format!("{target}\0"));
    let r = D3DCompile(
        SHADER_SRC.as_ptr() as *const c_void,
        SHADER_SRC.len(),
        PCSTR(n.as_ptr()),
        None,
        None,
        PCSTR(e.as_ptr()),
        PCSTR(t.as_ptr()),
        D3DCOMPILE_OPTIMIZATION_LEVEL3 | extra_flags,
        0,
        &mut blob,
        Some(&mut err),
    );
    if let Err(er) = r {
        let m = err
            .map(|b| String::from_utf8_lossy(std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize())).into_owned())
            .unwrap_or_default();
        return Err(format!("shader {entry}: {er}: {m}"));
    }
    let b = blob.ok_or_else(|| format!("shader {entry}: no output"))?;
    Ok(std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()).to_vec())
}

pub(super) struct Renderer {
    pub g: Gpu,
    vs: ID3D11VertexShader,
    ps_crop: ID3D11PixelShader,
    ps_down: ID3D11PixelShader,
    ps_glass: ID3D11PixelShader,
    cb: ID3D11Buffer,
    smp: ID3D11SamplerState,
    smp_ani: ID3D11SamplerState,
}

impl Renderer {
    /// `strict` compiles the glass shader with IEEE strictness, so the shader's own "not finite" check (turned on by the offscreen path)
    /// is not optimised away. The live path does not check and does not need it.
    pub(super) unsafe fn new(g: Gpu, strict: bool) -> Result<Renderer, String> {
        let mkps = |entry: &str, flags: u32| -> Result<ID3D11PixelShader, String> {
            let b = compile(entry, "ps_5_0", flags)?;
            let mut ps = None;
            g.dev.CreatePixelShader(&b, None::<&ID3D11ClassLinkage>, Some(&mut ps)).map_err(|e| format!("CreatePixelShader {entry}: {e}"))?;
            ps.ok_or_else(|| format!("CreatePixelShader {entry}: no shader"))
        };
        let vsb = compile("VSMain", "vs_5_0", 0)?;
        let mut vs = None;
        g.dev.CreateVertexShader(&vsb, None::<&ID3D11ClassLinkage>, Some(&mut vs)).map_err(|e| format!("CreateVertexShader: {e}"))?;
        let ps_crop = mkps("PSCrop", 0)?;
        let ps_down = mkps("PSDown", 0)?;
        let ps_glass = mkps("PSGlass", if strict { D3DCOMPILE_IEEE_STRICTNESS } else { 0 })?;
        let (mut cbuf, mut smp, mut smp_ani) = (None, None, None);
        g.dev
            .CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: size_of::<Cb>() as u32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                    StructureByteStride: 0,
                },
                None,
                Some(&mut cbuf),
            )
            .map_err(|e| format!("CreateBuffer: {e}"))?;
        // s0: plain linear. s1: anisotropic, for the rim where the picture is squeezed in one direction.
        let sampler = |filter: D3D11_FILTER, aniso: u32| D3D11_SAMPLER_DESC {
            Filter: filter,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MaxAnisotropy: aniso,
            ComparisonFunc: D3D11_COMPARISON_NEVER,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        g.dev.CreateSamplerState(&sampler(D3D11_FILTER_MIN_MAG_MIP_LINEAR, 1), Some(&mut smp)).map_err(|e| format!("CreateSamplerState: {e}"))?;
        g.dev.CreateSamplerState(&sampler(D3D11_FILTER_ANISOTROPIC, 16), Some(&mut smp_ani)).map_err(|e| format!("CreateSamplerState: {e}"))?;
        match (vs, cbuf, smp, smp_ani) {
            (Some(vs), Some(cb), Some(smp), Some(smp_ani)) => Ok(Renderer { g, vs, ps_crop, ps_down, ps_glass, cb, smp, smp_ani }),
            _ => Err("D3D11 returned no object".into()),
        }
    }

    unsafe fn pass(&self, ps: &ID3D11PixelShader, rtv: &ID3D11RenderTargetView, w: u32, h: u32, srv: &ID3D11ShaderResourceView, cb: &Cb) {
        let c = &self.g.ctx;
        c.PSSetShaderResources(0, Some(&[None])); // never bound for reading and writing at once
        c.UpdateSubresource(&self.cb, 0, None, cb as *const Cb as *const c_void, 0, 0);
        c.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None::<&ID3D11DepthStencilView>);
        c.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: w as f32, Height: h as f32, MinDepth: 0.0, MaxDepth: 1.0 }]));
        c.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        c.VSSetShader(&self.vs, None);
        c.PSSetShader(ps, None);
        c.PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
        c.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
        c.PSSetSamplers(0, Some(&[Some(self.smp.clone()), Some(self.smp_ani.clone())]));
        c.Draw(3, 0);
    }

    /// Unbind the render target and the texture, so a later resize or release of the swap chain's buffer is not held up by this context.
    pub(super) unsafe fn unbind(&self) {
        self.g.ctx.PSSetShaderResources(0, Some(&[None]));
        self.g.ctx.OMSetRenderTargets(None, None::<&ID3D11DepthStencilView>);
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------
// Textures
// ---------------------------------------------------------------------------------------------------------------------------------

unsafe fn tex2d(g: &Gpu, w: u32, h: u32, mips: u32, fmt: DXGI_FORMAT, bind: u32) -> R<ID3D11Texture2D> {
    let mut t = None;
    g.dev.CreateTexture2D(
        &D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: mips,
            ArraySize: 1,
            Format: fmt,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        },
        None,
        Some(&mut t),
    )?;
    t.ok_or_else(fail)
}

unsafe fn rtv_mip(g: &Gpu, t: &ID3D11Texture2D, fmt: DXGI_FORMAT, mip: u32) -> R<ID3D11RenderTargetView> {
    let mut v = None;
    g.dev.CreateRenderTargetView(
        t,
        Some(&D3D11_RENDER_TARGET_VIEW_DESC {
            Format: fmt,
            ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_RTV { MipSlice: mip } },
        }),
        Some(&mut v),
    )?;
    v.ok_or_else(fail)
}

unsafe fn srv_mip(g: &Gpu, t: &ID3D11Texture2D, fmt: DXGI_FORMAT, mip: u32, count: u32) -> R<ID3D11ShaderResourceView> {
    let mut v = None;
    g.dev.CreateShaderResourceView(
        t,
        Some(&D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: fmt,
            ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: mip, MipLevels: count } },
        }),
        Some(&mut v),
    )?;
    v.ok_or_else(fail)
}

/// A BGRA8 texture the glass reads (the mirror of a monitor, or a picture the caller gave). `data` is tightly packed BGRA.
pub(super) struct Source {
    pub tex: ID3D11Texture2D,
    pub srv: ID3D11ShaderResourceView,
    pub w: u32,
    pub h: u32,
}

impl Source {
    pub(super) unsafe fn new(g: &Gpu, w: u32, h: u32, data: Option<&[u8]>) -> R<Source> {
        let tex = tex2d(g, w, h, 1, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_BIND_SHADER_RESOURCE.0 as u32)?;
        if let Some(d) = data {
            g.ctx.UpdateSubresource(&tex, 0, None, d.as_ptr() as *const c_void, w * 4, 0);
        }
        let srv = srv_mip(g, &tex, DXGI_FORMAT_B8G8R8A8_UNORM, 0, 1)?;
        Ok(Source { tex, srv, w, h })
    }
}

/// The backdrop of one glass: the part of the source around the glass (its rectangle plus the padding), copied out and followed by
/// a blur pyramid. Gamma encoded values, 10 bits per channel (half float would be four times the memory for no visible gain).
pub(super) struct Region {
    /// Allocated size of level 0, device px: a multiple of 256, at least the glass plus padding on every side.
    pub w: u32,
    pub h: u32,
    /// Padding around the glass, device px.
    pub pad: u32,
    levels: u32,
    tex: ID3D11Texture2D,
    rtv: Vec<ID3D11RenderTargetView>,
    srv: Vec<ID3D11ShaderResourceView>,
    full_srv: ID3D11ShaderResourceView,
    guard_level: u32,
    staging: ID3D11Texture2D,
}

/// Size of the region texture for a glass of `w` x `h` device px.
pub(super) fn region_size(w: u32, h: u32, pad: u32) -> (u32, u32) {
    let up = |n: u32| n.div_ceil(REGION_STEP).max(1) * REGION_STEP;
    (up(w + 2 * pad), up(h + 2 * pad))
}

impl Region {
    pub(super) unsafe fn new(g: &Gpu, w: u32, h: u32, pad: u32) -> R<Region> {
        let fmt = DXGI_FORMAT_R10G10B10A2_UNORM;
        let (rw, rh) = region_size(w, h, pad);
        // levels as in the experiment (nine), fewer only if the texture is too small for them
        let levels = MAX_LEVELS.min(32 - rw.min(rh).leading_zeros());
        let tex = tex2d(g, rw, rh, levels, fmt, (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32)?;
        let (mut rtv, mut srv) = (vec![], vec![]);
        for m in 0..levels {
            rtv.push(rtv_mip(g, &tex, fmt, m)?);
            srv.push(srv_mip(g, &tex, fmt, m, 1)?);
        }
        let full_srv = srv_mip(g, &tex, fmt, 0, levels)?;
        let guard_level = (0..levels).find(|&l| (rw >> l) <= GUARD_WIDTH).unwrap_or(levels - 1);
        let mut d = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut d);
        d.Width = (rw >> guard_level).max(1);
        d.Height = (rh >> guard_level).max(1);
        d.MipLevels = 1;
        d.Usage = D3D11_USAGE_STAGING;
        d.BindFlags = 0;
        d.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        d.MiscFlags = 0;
        let mut st = None;
        g.dev.CreateTexture2D(&d, None, Some(&mut st))?;
        let staging = st.ok_or_else(fail)?;
        Ok(Region { w: rw, h: rh, pad, levels, tex, rtv, srv, full_srv, guard_level, staging })
    }

    /// Whether this region is the right one for a glass of this size, or has to be allocated again.
    pub(super) fn fits(&self, w: u32, h: u32, pad: u32) -> bool {
        self.pad == pad && (self.w, self.h) == region_size(w, h, pad)
    }

    /// A fingerprint of the backdrop as the glass will see it: one coarse level of the pyramid (the first at most 160 px wide), only the
    /// part that belongs to this glass and its padding. Blocks until the GPU has finished the pyramid.
    pub(super) unsafe fn fingerprint(&self, g: &Gpu, glass_w: u32, glass_h: u32) -> R<u64> {
        let l = self.guard_level;
        g.ctx.CopySubresourceRegion(&self.staging, 0, 0, 0, 0, &self.tex, l, None);
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        g.ctx.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
        let (lw, lh) = ((self.w >> l).max(1), (self.h >> l).max(1));
        // only the glass and its padding, rounded up to whole texels of this level
        let used = |n: u32, full: u32| (((n + 2 * self.pad) + (1 << l) - 1) >> l).min(full);
        let (uw, uh) = (used(glass_w, lw), used(glass_h, lh));
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for y in 0..uh {
            let row = std::slice::from_raw_parts((m.pData as *const u8).add((y * m.RowPitch) as usize), (uw * 4) as usize);
            let (words, _) = row.as_chunks::<4>();
            for px in words {
                h = (h ^ u32::from_le_bytes(*px) as u64).wrapping_mul(0x0100_0000_01b3);
            }
        }
        g.ctx.Unmap(&self.staging, 0);
        Ok(h)
    }
}

fn fail() -> windows::core::Error {
    windows::Win32::Foundation::E_FAIL.into()
}

// ---------------------------------------------------------------------------------------------------------------------------------
// Passes
// ---------------------------------------------------------------------------------------------------------------------------------

/// A draw prepared by `prep`: the constant buffer that the glass pass needs too.
pub(super) struct Prepared {
    cb: Cb,
}

/// Passes 1 and 2: copy the glass rectangle plus padding out of the source, then build the blur pyramid.
pub(super) unsafe fn prep(r: &Renderer, region: &Region, src: &Source, geo: &Geometry, look: &Look, check: bool) -> Prepared {
    let mut cb = make_cb(geo, look, (src.w, src.h), region, check);
    r.pass(&r.ps_crop, &region.rtv[0], region.w, region.h, &src.srv, &cb);
    for m in 1..region.levels as usize {
        let (sw, sh) = ((region.w >> (m - 1)).max(1), (region.h >> (m - 1)).max(1));
        let (dw, dh) = ((region.w >> m).max(1), (region.h >> m).max(1));
        cb.pass = [1.0 / sw as f32, 1.0 / sh as f32, dw as f32, dh as f32];
        r.pass(&r.ps_down, &region.rtv[m], dw, dh, &region.srv[m - 1], &cb);
    }
    Prepared { cb }
}

/// Pass 3: the glass into `rtv`, which is `geo.w` x `geo.h`.
pub(super) unsafe fn glass_pass(r: &Renderer, region: &Region, p: &Prepared, geo: &Geometry, rtv: &ID3D11RenderTargetView) {
    r.pass(&r.ps_glass, rtv, geo.w, geo.h, &region.full_srv, &p.cb);
    r.unbind();
}

// ---------------------------------------------------------------------------------------------------------------------------------
// Offscreen
// ---------------------------------------------------------------------------------------------------------------------------------

/// Copy a whole texture into a staging texture and return the bytes, `bpp` bytes per pixel, tightly packed.
unsafe fn readback(g: &Gpu, tex: &ID3D11Texture2D, w: u32, h: u32, bpp: u32) -> R<Vec<u8>> {
    let mut d = D3D11_TEXTURE2D_DESC::default();
    tex.GetDesc(&mut d);
    d.Usage = D3D11_USAGE_STAGING;
    d.BindFlags = 0;
    d.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    d.MiscFlags = 0;
    let mut st = None;
    g.dev.CreateTexture2D(&d, None, Some(&mut st))?;
    let st = st.ok_or_else(fail)?;
    g.ctx.CopyResource(&st, tex);
    let mut m = D3D11_MAPPED_SUBRESOURCE::default();
    g.ctx.Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
    let mut out = Vec::with_capacity((w * h * bpp) as usize);
    for y in 0..h {
        out.extend_from_slice(std::slice::from_raw_parts((m.pData as *const u8).add((y * m.RowPitch) as usize), (w * bpp) as usize));
    }
    g.ctx.Unmap(&st, 0);
    Ok(out)
}

/// The largest picture or rectangle the offscreen path accepts, per side.
const OFFSCREEN_MAX: u32 = 16384;

/// What only the tests change about an offscreen picture.
pub(super) struct Opts {
    pub look: Look,
    /// Skip the hardware try and use WARP (what a CI runner ends up with).
    pub force_warp: bool,
}

/// Draw the glass over a picture into a private texture. `src_bgra` is tightly packed BGRA8, `rect` the glass in picture px. Returns the
/// glass's pixels, premultiplied BGRA, `rect` sized. `opts` is for the tests; the public function passes the shipped look and lets the
/// hardware go first.
pub(super) fn render(src_bgra: &[u8], src_w: u32, src_h: u32, rect: (i32, i32, u32, u32), radius_px: f32, scale: f32, opts: &Opts) -> Result<Vec<u8>, String> {
    let (look, force_warp) = (&opts.look, opts.force_warp);
    let (x, y, w, h) = rect;
    if src_w == 0 || src_h == 0 || w == 0 || h == 0 || src_w > OFFSCREEN_MAX || src_h > OFFSCREEN_MAX || w > OFFSCREEN_MAX || h > OFFSCREEN_MAX {
        return Err(format!("picture {src_w}x{src_h} or rectangle {w}x{h} is empty or too large"));
    }
    if src_bgra.len() as u64 != src_w as u64 * src_h as u64 * 4 {
        return Err(format!("the picture has {} bytes, {src_w}x{src_h} BGRA needs {}", src_bgra.len(), src_w as u64 * src_h as u64 * 4));
    }
    if !(scale.is_finite() && scale > 0.0 && radius_px.is_finite() && radius_px >= 0.0) {
        return Err("scale must be positive and the radius not negative".into());
    }
    let geo = Geometry { w, h, origin: (x, y), scale, radius_px, cut_corners: true };
    unsafe {
        let r = offscreen_renderer(force_warp)?;
        render_on(&r, src_bgra, src_w, src_h, &geo, look)
    }
}

/// A device and the compiled shaders for offscreen pictures: hardware if there is any (unless `force_warp`), else WARP.
pub(super) unsafe fn offscreen_renderer(force_warp: bool) -> Result<Renderer, String> {
    let gpu = Gpu::offscreen(force_warp).map_err(|e| format!("no D3D11 device, not even WARP: {e}"))?;
    Renderer::new(gpu, true)
}

/// The drawing of `render` on a renderer the caller holds (arguments already checked).
pub(super) unsafe fn render_on(r: &Renderer, src_bgra: &[u8], src_w: u32, src_h: u32, geo: &Geometry, look: &Look) -> Result<Vec<u8>, String> {
    let (w, h, scale) = (geo.w, geo.h, geo.scale);
    {
        let e = |what: &'static str| move |er: windows::core::Error| format!("{what}: {er}");
        let src = Source::new(&r.g, src_w, src_h, Some(src_bgra)).map_err(e("source texture"))?;
        let region = Region::new(&r.g, w, h, region_pad(scale)).map_err(e("region texture"))?;
        let prepared = prep(r, &region, &src, geo, look, true);
        let off = tex2d(&r.g, w, h, 1, DXGI_FORMAT_B8G8R8A8_UNORM, (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32)
            .map_err(e("target texture"))?;
        let rtv = rtv_mip(&r.g, &off, DXGI_FORMAT_B8G8R8A8_UNORM, 0).map_err(e("target view"))?;
        r.g.ctx.ClearRenderTargetView(&rtv, &[0.0; 4]);
        glass_pass(r, &region, &prepared, geo, &rtv);
        let out = readback(&r.g, &off, w, h, 4).map_err(e("read back"))?;
        if let Err(er) = r.g.dev.GetDeviceRemovedReason() {
            return Err(format!("the device was removed while drawing: {er}"));
        }
        Ok(out)
    }
}
