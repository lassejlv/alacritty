//! Clipped Kitty image quads and a bounded texture cache shared across a tab's panes.

use std::collections::HashMap;
use std::sync::Arc;
use std::{mem, ptr};

use alacritty_terminal::graphics::KittyGraphicsRenderPlacement as Placement;
use alacritty_terminal::graphics::media::{GraphicsImage, graphics_display_layout};
use alacritty_terminal::grid::Dimensions;

use crate::geometry::SizeInfo;
use crate::gl;
use crate::gl::types::*;
use crate::shader::{ShaderError, ShaderProgram, ShaderVersion};

const CACHE_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
struct Rect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}
impl Rect {
    fn intersection(self, other: Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let width = (self.x + self.width).min(other.x + other.width) - x;
        let height = (self.y + self.height).min(other.y + other.height) - y;
        (width > 0. && height > 0.).then_some(Self { x, y, width, height })
    }
}

#[derive(Debug)]
struct Tile {
    texture: GLuint,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}
impl Drop for Tile {
    fn drop(&mut self) {
        // SAFETY: The owning renderer is used/dropped only with its context current.
        unsafe {
            gl::DeleteTextures(1, &self.texture);
        }
    }
}

#[derive(Debug)]
struct CachedImage {
    // Keeping the immutable image alive prevents pointer reuse from aliasing a cache key.
    _image: Arc<GraphicsImage>,
    tiles: Vec<Tile>,
    bytes: usize,
    used: u64,
}

#[derive(Debug)]
pub struct GraphicsRenderer {
    program: ShaderProgram,
    viewport: GLint,
    vao: GLuint,
    vbo: GLuint,
    max_tile: u32,
    clock: u64,
    bytes: usize,
    cache: HashMap<usize, CachedImage>,
}

impl GraphicsRenderer {
    pub fn new(version: ShaderVersion) -> Result<Self, ShaderError> {
        let program = ShaderProgram::new(
            version,
            None,
            include_str!("../shaders/graphics.v.glsl"),
            include_str!("../shaders/graphics.f.glsl"),
        )?;
        let viewport = program.get_uniform_location(c"viewport")?;
        let sampler = program.get_uniform_location(c"image")?;
        let (mut vao, mut vbo, mut limit) = (0, 0, 0);
        // SAFETY: Initialized GL entry points and the current owning context; outputs are live.
        unsafe {
            let attribute = gl::GetAttribLocation(program.id(), c"vertex".as_ptr()) as GLuint;
            gl::GenVertexArrays(1, &mut vao);
            gl::GenBuffers(1, &mut vbo);
            gl::BindVertexArray(vao);
            gl::BindBuffer(gl::ARRAY_BUFFER, vbo);
            gl::EnableVertexAttribArray(attribute);
            gl::VertexAttribPointer(attribute, 4, gl::FLOAT, gl::FALSE, 16, ptr::null());
            gl::BindVertexArray(0);
            gl::BindBuffer(gl::ARRAY_BUFFER, 0);
            gl::GetIntegerv(gl::MAX_TEXTURE_SIZE, &mut limit);
            gl::UseProgram(program.id());
            // Unit 1 keeps the text renderer's cached atlas binding on unit 0 intact.
            gl::Uniform1i(sampler, 1);
            gl::UseProgram(0);
        };
        Ok(Self {
            program,
            viewport,
            vao,
            vbo,
            max_tile: (limit - 2).clamp(1, 4096) as u32,
            clock: 0,
            bytes: 0,
            cache: HashMap::new(),
        })
    }

    pub fn prune(&mut self) {
        self.cache.retain(|_, cached| {
            if Arc::strong_count(&cached._image) == 1 {
                self.bytes -= cached.bytes;
                false
            } else {
                true
            }
        });
    }

    fn prepare(&mut self, image: &Arc<GraphicsImage>) -> Option<usize> {
        self.clock = self.clock.wrapping_add(1);
        let key = Arc::as_ptr(image) as usize;
        if let Some(cached) = self.cache.get_mut(&key) {
            cached.used = self.clock;
            return Some(key);
        }
        let pixels = image.rgba()?;
        let mut tiles = Vec::new();
        let mut bytes = 0;
        for y in (0..image.height).step_by(self.max_tile as usize) {
            for x in (0..image.width).step_by(self.max_tile as usize) {
                let width = (image.width - x).min(self.max_tile);
                let height = (image.height - y).min(self.max_tile);
                let data = padded_tile(pixels, image.width, image.height, x, y, width, height);
                bytes += data.len();
                let mut texture = 0;
                // SAFETY: Pixel storage matches the declared RGBA dimensions, and the GL
                // driver copies it synchronously. All texture handles belong to this context.
                unsafe {
                    gl::GenTextures(1, &mut texture);
                    gl::BindTexture(gl::TEXTURE_2D, texture);
                    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as GLint);
                    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as GLint);
                    gl::TexParameteri(
                        gl::TEXTURE_2D,
                        gl::TEXTURE_WRAP_S,
                        gl::CLAMP_TO_EDGE as GLint,
                    );
                    gl::TexParameteri(
                        gl::TEXTURE_2D,
                        gl::TEXTURE_WRAP_T,
                        gl::CLAMP_TO_EDGE as GLint,
                    );
                    gl::PixelStorei(gl::UNPACK_ALIGNMENT, 1);
                    gl::TexImage2D(
                        gl::TEXTURE_2D,
                        0,
                        gl::RGBA as GLint,
                        (width + 2) as GLsizei,
                        (height + 2) as GLsizei,
                        0,
                        gl::RGBA,
                        gl::UNSIGNED_BYTE,
                        data.as_ptr().cast(),
                    );
                }
                tiles.push(Tile { texture, x, y, width, height });
            }
        }
        self.bytes += bytes;
        self.cache.insert(key, CachedImage {
            _image: image.clone(),
            tiles,
            bytes,
            used: self.clock,
        });
        while self.cache.len() > 256 || (self.bytes > CACHE_BYTES && self.cache.len() > 1) {
            let oldest = self
                .cache
                .iter()
                .filter(|(id, _)| **id != key)
                .min_by_key(|(_, image)| image.used)
                .map(|(id, _)| *id)
                .unwrap();
            if let Some(image) = self.cache.remove(&oldest) {
                self.bytes -= image.bytes;
            }
        }
        Some(key)
    }

    fn flush(&self, texture: GLuint, vertices: &mut Vec<f32>) {
        if vertices.is_empty() {
            return;
        }
        // SAFETY: This initialized buffer contains four floats per vertex. Upload and draw
        // use our current context, VAO, VBO, program, and a live cached texture.
        unsafe {
            gl::BindTexture(gl::TEXTURE_2D, texture);
            gl::BufferData(
                gl::ARRAY_BUFFER,
                (vertices.len() * mem::size_of::<f32>()) as isize,
                vertices.as_ptr().cast(),
                gl::STREAM_DRAW,
            );
            gl::DrawArrays(gl::TRIANGLES, 0, (vertices.len() / 4) as GLsizei);
        }
        vertices.clear();
    }

    pub fn draw<'a>(&mut self, size: &SizeInfo, placements: impl Iterator<Item = &'a Placement>) {
        // SAFETY: All operations target the current renderer's own GL objects. The pane
        // viewport/scissor remains installed by Display and clips neighboring panes.
        unsafe {
            gl::UseProgram(self.program.id());
            gl::Uniform2f(self.viewport, size.width(), size.height());
            gl::ActiveTexture(gl::TEXTURE1);
            gl::BindVertexArray(self.vao);
            gl::BindBuffer(gl::ARRAY_BUFFER, self.vbo);
            gl::BlendFuncSeparate(
                gl::SRC_ALPHA,
                gl::ONE_MINUS_SRC_ALPHA,
                gl::ONE,
                gl::ONE_MINUS_SRC_ALPHA,
            );
        }
        let mut batch = Vec::new();
        let mut batch_texture = 0;
        for placement in placements {
            let Some((destination, source)) = geometry(placement, size) else { continue };
            // New uploads can evict old textures; finish any pending draw before that.
            if !self.cache.contains_key(&(Arc::as_ptr(&placement.image) as usize)) {
                self.flush(batch_texture, &mut batch);
            }
            let Some(key) = self.prepare(&placement.image) else { continue };
            let cached = &self.cache[&key];
            for tile in &cached.tiles {
                let tile_rect = Rect {
                    x: tile.x as f32,
                    y: tile.y as f32,
                    width: tile.width as f32,
                    height: tile.height as f32,
                };
                let Some(visible) = source.intersection(tile_rect) else { continue };
                let x = destination.x + (visible.x - source.x) / source.width * destination.width;
                let y = destination.y + (visible.y - source.y) / source.height * destination.height;
                let right = x + visible.width / source.width * destination.width;
                let bottom = y + visible.height / source.height * destination.height;
                let u = (visible.x - tile.x as f32 + 1.) / (tile.width + 2) as f32;
                let v = (visible.y - tile.y as f32 + 1.) / (tile.height + 2) as f32;
                let ur = u + visible.width / (tile.width + 2) as f32;
                let vb = v + visible.height / (tile.height + 2) as f32;
                let vertices: [f32; 24] = [
                    x, y, u, v, right, y, ur, v, x, bottom, u, vb, x, bottom, u, vb, right, y, ur,
                    v, right, bottom, ur, vb,
                ];
                if batch_texture != tile.texture {
                    self.flush(batch_texture, &mut batch);
                    batch_texture = tile.texture;
                }
                batch.extend_from_slice(&vertices);
                if batch.len() >= 24 * 1024 {
                    self.flush(batch_texture, &mut batch);
                }
            }
        }
        self.flush(batch_texture, &mut batch);
        // SAFETY: Restore the bindings and blend mode expected by existing text rendering.
        unsafe {
            gl::BindTexture(gl::TEXTURE_2D, 0);
            gl::ActiveTexture(gl::TEXTURE0);
            gl::BindVertexArray(0);
            gl::BindBuffer(gl::ARRAY_BUFFER, 0);
            gl::UseProgram(0);
            gl::BlendFunc(gl::SRC1_COLOR, gl::ONE_MINUS_SRC1_COLOR);
        }
    }
}

impl Drop for GraphicsRenderer {
    fn drop(&mut self) {
        // SAFETY: Display makes this context current before destroying its renderer.
        unsafe {
            gl::DeleteBuffers(1, &self.vbo);
            gl::DeleteVertexArrays(1, &self.vao);
        }
    }
}

/// Return the visible destination and its matching pixel crop from the source image.
fn geometry(p: &Placement, size: &SizeInfo) -> Option<(Rect, Rect)> {
    let (cw, ch) = (size.cell_width(), size.cell_height());
    let virtual_placement = p.virtual_cell.is_some();
    let layout = graphics_display_layout(
        p.source_width,
        p.source_height,
        p.display_cols,
        p.display_rows,
        (cw, ch),
        (p.x_offset, p.y_offset),
        virtual_placement,
    );
    let cell_x = size.padding_x() + (p.col as f32 + p.col_offset as f32) * cw;
    let cell_y = size.padding_y() + p.viewport_row as f32 * ch;
    let (offset_x, offset_y) =
        p.virtual_cell.map_or((0., 0.), |(col, row)| (col as f32 * cw, row as f32 * ch));
    let destination = Rect {
        x: cell_x + p.x_offset as f32 + layout.image_offset.0 - offset_x,
        y: cell_y + p.y_offset as f32 + layout.image_offset.1 - offset_y,
        width: layout.image_size.0,
        height: layout.image_size.1,
    };
    let viewport = Rect {
        x: size.padding_x(),
        y: size.padding_y(),
        width: size.columns() as f32 * cw,
        height: size.screen_lines() as f32 * ch,
    };
    let clip = if virtual_placement {
        Rect { x: cell_x, y: cell_y, width: cw, height: ch }
    } else {
        Rect {
            x: cell_x,
            y: cell_y + p.clip_top_rows as f32 * ch,
            width: p.occupied_cols as f32 * cw,
            height: p.occupied_rows.saturating_sub(p.clip_top_rows + p.clip_bottom_rows) as f32
                * ch,
        }
    };
    let visible = destination.intersection(viewport)?.intersection(clip)?;
    let source = Rect {
        x: p.source_x as f32
            + (visible.x - destination.x) / destination.width * p.source_width as f32,
        y: p.source_y as f32
            + (visible.y - destination.y) / destination.height * p.source_height as f32,
        width: visible.width / destination.width * p.source_width as f32,
        height: visible.height / destination.height * p.source_height as f32,
    };
    Some((visible, source))
}

/// One texel of neighboring pixels prevents linear-filter seams between tiles.
fn padded_tile(
    pixels: &[u8],
    image_width: u32,
    image_height: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let mut output = Vec::with_capacity((width + 2) as usize * (height + 2) as usize * 4);
    for row in 0..height + 2 {
        let sy = (y + row).saturating_sub(1).min(image_height - 1);
        for col in 0..width + 2 {
            let sx = (x + col).saturating_sub(1).min(image_width - 1);
            let offset = (sy as usize * image_width as usize + sx as usize) * 4;
            output.extend_from_slice(&pixels[offset..offset + 4]);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement() -> Placement {
        Placement {
            placement_serial: 1,
            image_id: 1,
            placement_id: 1,
            image: Arc::new(GraphicsImage::from_rgba(20, 20, vec![255; 1600])),
            image_width: 20,
            image_height: 20,
            image_generation: 1,
            animation_deadline: None,
            viewport_row: 0,
            col: 0,
            col_offset: 0,
            virtual_cell: None,
            source_x: 0,
            source_y: 0,
            source_width: 20,
            source_height: 20,
            display_cols: None,
            display_rows: None,
            occupied_cols: 2,
            occupied_rows: 1,
            clip_top_rows: 0,
            clip_bottom_rows: 0,
            x_offset: 0,
            y_offset: 0,
            z_index: 0,
        }
    }
    fn size() -> SizeInfo {
        SizeInfo::new(104., 84., 10., 20., 2., 2., false)
    }
    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 0.001, "{a} != {b}");
    }

    #[test]
    fn placement_crop_tracks_negative_origins_and_text_viewport_clipping() {
        let mut p = placement();
        p.col_offset = -1;
        let (destination, source) = geometry(&p, &size()).unwrap();
        close(destination.x, 2.);
        close(destination.width, 10.);
        close(source.x, 10.);
        close(source.width, 10.);
        p.viewport_row = -1;
        assert!(geometry(&p, &size()).is_none());
    }

    #[test]
    fn letterboxed_virtual_cells_sample_only_their_piece_of_the_image() {
        let mut p = placement();
        p.display_cols = Some(4);
        p.display_rows = Some(1);
        p.virtual_cell = Some((0, 0));
        p.occupied_cols = 1;
        // The square image occupies the middle two cells of the four-cell box.
        assert!(geometry(&p, &size()).is_none());
        p.col = 1;
        p.virtual_cell = Some((1, 0));
        let (destination, source) = geometry(&p, &size()).unwrap();
        close(destination.x, 12.);
        close(destination.width, 10.);
        close(source.x, 0.);
        close(source.width, 10.);
        p.col = 2;
        p.virtual_cell = Some((2, 0));
        close(geometry(&p, &size()).unwrap().1.x, 10.);
    }

    #[test]
    fn partial_margin_clipping_crops_pixels_without_rescaling() {
        let mut p = placement();
        p.display_rows = Some(2);
        p.occupied_cols = 4;
        p.occupied_rows = 2;
        p.clip_top_rows = 1;
        let (destination, source) = geometry(&p, &size()).unwrap();
        close(destination.y, 22.);
        close(destination.height, 20.);
        close(source.y, 10.);
        close(source.height, 10.);
    }

    #[test]
    fn texture_tiles_repeat_edges_and_copy_neighbors_at_internal_seams() {
        let pixels = [255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 255];
        let tile = padded_tile(&pixels, 3, 1, 1, 0, 1, 1);
        assert_eq!(tile, pixels.repeat(3));
        let edge = padded_tile(&pixels[..4], 1, 1, 0, 0, 1, 1);
        assert_eq!(edge, pixels[..4].repeat(9));
    }
}
