//! egui drawn through SDL3's renderer: its meshes as `SDL_RenderGeometry`
//! triangles, its textures (the font atlas, any images) as SDL textures.
//! Our own glue: the `egui-sdl3` crate is pinned to older sdl3 and egui
//! and blends egui's premultiplied colours as straight alpha.
//!
//! egui's colours and textures are premultiplied by alpha, so both are
//! drawn with SDL's premultiplied blend. Coordinates are the renderer's
//! output pixels: egui's points times its pixels per point.

use egui::epaint::{ImageDelta, Primitive};
use sdl3::pixels::{FColor, PixelFormat};
use sdl3::rect::Rect;
use sdl3::render::{Canvas, FPoint, ScaleMode, Texture, TextureCreator, Vertex};
use sdl3::sys::blendmode::SDL_BLENDMODE_BLEND_PREMULTIPLIED;
use sdl3::video::{Window, WindowContext};
use std::collections::HashMap;

/// egui's textures, as SDL holds them.
pub struct Painter<'c> {
    creator: &'c TextureCreator<WindowContext>,
    textures: HashMap<egui::TextureId, Texture<'c>>,
    /// Reused for each mesh's vertices.
    vertices: Vec<Vertex>,
}

impl<'c> Painter<'c> {
    pub fn new(creator: &'c TextureCreator<WindowContext>) -> Painter<'c> {
        Painter {
            creator,
            textures: HashMap::new(),
            vertices: Vec::new(),
        }
    }

    /// Draw egui's `output`, from a pass of `ctx`, on `canvas`.
    pub fn paint(
        &mut self,
        canvas: &mut Canvas<Window>,
        ctx: &egui::Context,
        mut output: egui::FullOutput,
    ) -> Result<(), String> {
        let ppp = output.pixels_per_point;
        // Drained: epaint asserts (in debug builds) that every delta was
        // taken.
        let set: Vec<_> = output.textures_delta.set.drain().collect();
        let free: Vec<_> = output.textures_delta.free.drain().collect();
        for (id, deltas) in &set {
            for delta in deltas {
                self.set(*id, delta)?;
            }
        }
        // Untextured triangles take the renderer's blend.
        unsafe {
            sdl3::sys::render::SDL_SetRenderDrawBlendMode(
                canvas.raw(),
                SDL_BLENDMODE_BLEND_PREMULTIPLIED,
            );
        }
        let (w, h) = canvas.output_size().map_err(|e| e.to_string())?;
        for job in ctx.tessellate(output.shapes, ppp) {
            let Primitive::Mesh(mesh) = job.primitive else {
                continue;
            };
            let min = (job.clip_rect.min * ppp).round();
            let max = (job.clip_rect.max * ppp).round();
            let (x0, y0) = (min.x.clamp(0.0, w as f32), min.y.clamp(0.0, h as f32));
            let (x1, y1) = (max.x.clamp(x0, w as f32), max.y.clamp(y0, h as f32));
            if x1 <= x0 || y1 <= y0 || mesh.indices.is_empty() {
                continue;
            }
            canvas.set_clip_rect(Rect::new(
                x0 as i32,
                y0 as i32,
                (x1 - x0) as u32,
                (y1 - y0) as u32,
            ));
            self.vertices.clear();
            self.vertices.extend(mesh.vertices.iter().map(|v| {
                let [r, g, b, a] = v.color.to_array().map(|c| f32::from(c) / 255.0);
                Vertex {
                    position: FPoint::new(v.pos.x * ppp, v.pos.y * ppp),
                    color: FColor::RGBA(r, g, b, a),
                    tex_coord: FPoint::new(v.uv.x, v.uv.y),
                }
            }));
            canvas
                .render_geometry(
                    &self.vertices,
                    self.textures.get(&mesh.texture_id),
                    &mesh.indices,
                )
                .map_err(|e| format!("drawing the menu: {e}"))?;
        }
        canvas.set_clip_rect(None);
        for id in &free {
            self.textures.remove(id);
        }
        Ok(())
    }

    /// Make or patch texture `id` as `delta` says.
    fn set(&mut self, id: egui::TextureId, delta: &ImageDelta) -> Result<(), String> {
        let egui::ImageData::Color(image) = &delta.image;
        let [w, h] = image.size.map(|n| n as u32);
        let err = |e: &dyn std::fmt::Display| format!("the menu's texture: {e}");
        if delta.pos.is_none() {
            // A whole new image: a new texture.
            let mut t = self
                .creator
                .create_texture_static(PixelFormat::ABGR8888, w, h)
                .map_err(|e| err(&e))?;
            unsafe {
                sdl3::sys::render::SDL_SetTextureBlendMode(
                    t.raw(),
                    SDL_BLENDMODE_BLEND_PREMULTIPLIED,
                );
            }
            t.set_scale_mode(match delta.options.magnification {
                egui::TextureFilter::Nearest => ScaleMode::Nearest,
                egui::TextureFilter::Linear => ScaleMode::Linear,
            });
            self.textures.insert(id, t);
        }
        let t = self
            .textures
            .get_mut(&id)
            .ok_or_else(|| err(&"patched before it was made"))?;
        let rect = delta.pos.map(|[x, y]| Rect::new(x as i32, y as i32, w, h));
        // ABGR8888 is R, G, B, A in memory on a little-endian host, as
        // egui's Color32 is laid out.
        let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
        t.update(rect, &bytes, w as usize * 4).map_err(|e| err(&e))
    }
}
