use anyhow::{Context as _, Result};
use collections::FxHashMap;
use derive_more::{Deref, DerefMut};
use etagere::BucketedAtlasAllocator;
use gpui::{
    AtlasKey, AtlasTextureId, AtlasTextureKind, AtlasTextureList, AtlasTile, Bounds, DevicePixels,
    PlatformAtlas, Point, Size,
};
use metal::Device;
use parking_lot::Mutex;
use std::borrow::Cow;

pub(crate) struct MetalAtlas(Mutex<MetalAtlasState>);

impl MetalAtlas {
    pub(crate) fn new(device: Device, is_apple_gpu: bool) -> Self {
        MetalAtlas(Mutex::new(MetalAtlasState {
            device: AssertSend(device),
            is_apple_gpu,
            monochrome_textures: Default::default(),
            polychrome_textures: Default::default(),
            tiles_by_key: Default::default(),
        }))
    }

    pub(crate) fn metal_texture(&self, id: AtlasTextureId) -> metal::Texture {
        self.0.lock().texture(id).metal_texture.clone()
    }
}

struct MetalAtlasState {
    device: AssertSend<Device>,
    is_apple_gpu: bool,
    monochrome_textures: AtlasTextureList<MetalAtlasTexture>,
    polychrome_textures: AtlasTextureList<MetalAtlasTexture>,
    tiles_by_key: FxHashMap<AtlasKey, AtlasTile>,
}

impl PlatformAtlas for MetalAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: &AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        let mut lock = self.0.lock();
        if let Some(tile) = lock.tiles_by_key.get(key) {
            Ok(Some(*tile))
        } else {
            let Some((size, bytes)) = build()? else {
                return Ok(None);
            };
            let tile = lock
                .allocate(size, key.texture_kind())
                .context("failed to allocate")?;
            let texture = lock.texture(tile.texture_id);
            texture.upload(tile.bounds, &bytes);
            lock.tiles_by_key.insert(key.clone(), tile);
            Ok(Some(tile))
        }
    }

    fn remove(&self, key: &AtlasKey) {
        let mut lock = self.0.lock();
        let Some(tile) = lock.tiles_by_key.remove(key) else {
            return;
        };
        let id = tile.texture_id;

        let textures = match id.kind {
            AtlasTextureKind::Monochrome => &mut lock.monochrome_textures,
            AtlasTextureKind::Polychrome => &mut lock.polychrome_textures,
            AtlasTextureKind::Subpixel => unreachable!(),
        };

        let Some(texture_slot) = textures
            .textures
            .iter_mut()
            .find(|texture| texture.as_ref().is_some_and(|v| v.id == id))
        else {
            return;
        };

        if let Some(mut texture) = texture_slot.take() {
            texture.allocator.deallocate(tile.tile_id.into());
            texture.decrement_ref_count();
            if texture.is_unreferenced() {
                textures.free_list.push(id.index as usize);
            } else {
                *texture_slot = Some(texture);
            }
        }
    }
}

impl MetalAtlasState {
    fn allocate(
        &mut self,
        size: Size<DevicePixels>,
        texture_kind: AtlasTextureKind,
    ) -> Option<AtlasTile> {
        {
            let device = self.device.clone();
            let is_apple_gpu = self.is_apple_gpu;
            let textures = match texture_kind {
                AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
                AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
                AtlasTextureKind::Subpixel => unreachable!(),
            };

            if let Some(tile) = textures
                .iter_mut()
                .rev()
                .find_map(|texture| texture.allocate(size))
            {
                return Some(tile);
            }
            // Grow the last small page in place: tile coordinates and texture IDs stay valid.
            if let Some(texture) = textures.iter_mut().last() {
                while texture.grow(&device, is_apple_gpu, size) {
                    if let Some(tile) = texture.allocate(size) {
                        return Some(tile);
                    }
                }
            }
        }

        let texture = self.push_texture(size, texture_kind);
        texture.allocate(size)
    }

    fn push_texture(
        &mut self,
        min_size: Size<DevicePixels>,
        kind: AtlasTextureKind,
    ) -> &mut MetalAtlasTexture {
        const DEFAULT_ATLAS_SIZE: Size<DevicePixels> = Size {
            width: DevicePixels(256),
            height: DevicePixels(256),
        };
        // Max texture size on all modern Apple GPUs. Anything bigger than that crashes in validateWithDevice.
        const MAX_ATLAS_SIZE: Size<DevicePixels> = Size {
            width: DevicePixels(16384),
            height: DevicePixels(16384),
        };
        let texture_list = match kind {
            AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
            AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
            AtlasTextureKind::Subpixel => unreachable!(),
        };
        let page_size = texture_list
            .iter_mut()
            .fold(DEFAULT_ATLAS_SIZE, |size, texture| {
                size.max(&Size {
                    width: DevicePixels((texture.metal_texture.width() as i32).min(1024)),
                    height: DevicePixels((texture.metal_texture.height() as i32).min(1024)),
                })
            });
        let size = min_size.min(&MAX_ATLAS_SIZE).max(&page_size);
        let metal_texture = make_texture(&self.device, self.is_apple_gpu, size, kind);

        let index = texture_list.free_list.pop();

        let atlas_texture = MetalAtlasTexture {
            id: AtlasTextureId {
                index: index.unwrap_or(texture_list.textures.len()) as u32,
                kind,
            },
            allocator: etagere::BucketedAtlasAllocator::new(size_to_etagere(size)),
            metal_texture: AssertSend(metal_texture),
            live_atlas_keys: 0,
        };

        if let Some(ix) = index {
            texture_list.textures[ix] = Some(atlas_texture);
            texture_list.textures.get_mut(ix)
        } else {
            texture_list.textures.push(Some(atlas_texture));
            texture_list.textures.last_mut()
        }
        .unwrap()
        .as_mut()
        .unwrap()
    }

    fn texture(&self, id: AtlasTextureId) -> &MetalAtlasTexture {
        let textures = match id.kind {
            AtlasTextureKind::Monochrome => &self.monochrome_textures,
            AtlasTextureKind::Polychrome => &self.polychrome_textures,
            AtlasTextureKind::Subpixel => unreachable!(),
        };
        textures[id.index as usize].as_ref().unwrap()
    }
}

struct MetalAtlasTexture {
    id: AtlasTextureId,
    allocator: BucketedAtlasAllocator,
    metal_texture: AssertSend<metal::Texture>,
    live_atlas_keys: u32,
}

impl MetalAtlasTexture {
    fn grow(&mut self, device: &Device, is_apple_gpu: bool, min_size: Size<DevicePixels>) -> bool {
        let old_size = self.allocator.size();
        if min_size.width.0 > 1024 || min_size.height.0 > 1024 {
            return false;
        }
        let width = (old_size.width * 2)
            .max(min_size.width.0)
            .min(1024)
            .max(old_size.width);
        let height = (old_size.height * 2)
            .max(min_size.height.0)
            .min(1024)
            .max(old_size.height);
        if width == old_size.width && height == old_size.height {
            return false;
        }
        let texture = make_texture(
            device,
            is_apple_gpu,
            Size {
                width: DevicePixels(width),
                height: DevicePixels(height),
            },
            self.id.kind,
        );
        let bytes_per_row = old_size.width as usize * self.bytes_per_pixel() as usize;
        let mut bytes = vec![0u8; bytes_per_row * old_size.height as usize];
        let region = metal::MTLRegion::new_2d(0, 0, old_size.width as u64, old_size.height as u64);
        // Atlas textures are CPU-uploaded and GPU-read-only; in-flight commands retain the old texture.
        self.metal_texture
            .get_bytes(bytes.as_mut_ptr().cast(), bytes_per_row as u64, region, 0);
        texture.replace_region(region, 0, bytes.as_ptr().cast(), bytes_per_row as u64);
        self.metal_texture = AssertSend(texture);
        self.allocator.grow(etagere::size2(width, height));
        true
    }

    fn allocate(&mut self, size: Size<DevicePixels>) -> Option<AtlasTile> {
        let allocation = self.allocator.allocate(size_to_etagere(size))?;
        let tile = AtlasTile {
            texture_id: self.id,
            tile_id: allocation.id.into(),
            bounds: Bounds {
                origin: point_from_etagere(allocation.rectangle.min),
                size,
            },
            padding: 0,
        };
        self.live_atlas_keys += 1;
        Some(tile)
    }

    fn upload(&self, bounds: Bounds<DevicePixels>, bytes: &[u8]) {
        let region = metal::MTLRegion::new_2d(
            bounds.origin.x.into(),
            bounds.origin.y.into(),
            bounds.size.width.into(),
            bounds.size.height.into(),
        );
        self.metal_texture.replace_region(
            region,
            0,
            bytes.as_ptr() as *const _,
            bounds.size.width.to_bytes(self.bytes_per_pixel()) as u64,
        );
    }

    fn bytes_per_pixel(&self) -> u8 {
        use metal::MTLPixelFormat::*;
        match self.metal_texture.pixel_format() {
            A8Unorm | R8Unorm => 1,
            RGBA8Unorm | BGRA8Unorm => 4,
            _ => unimplemented!(),
        }
    }

    fn decrement_ref_count(&mut self) {
        self.live_atlas_keys -= 1;
    }

    fn is_unreferenced(&mut self) -> bool {
        self.live_atlas_keys == 0
    }
}

fn make_texture(
    device: &Device,
    is_apple_gpu: bool,
    size: Size<DevicePixels>,
    kind: AtlasTextureKind,
) -> metal::Texture {
    let descriptor = metal::TextureDescriptor::new();
    descriptor.set_width(size.width.into());
    descriptor.set_height(size.height.into());
    descriptor.set_pixel_format(match kind {
        AtlasTextureKind::Monochrome => metal::MTLPixelFormat::A8Unorm,
        AtlasTextureKind::Polychrome => metal::MTLPixelFormat::BGRA8Unorm,
        AtlasTextureKind::Subpixel => unreachable!(),
    });
    descriptor.set_usage(metal::MTLTextureUsage::ShaderRead);
    descriptor.set_storage_mode(if is_apple_gpu {
        metal::MTLStorageMode::Shared
    } else {
        metal::MTLStorageMode::Managed
    });
    device.new_texture(&descriptor)
}

fn size_to_etagere(size: Size<DevicePixels>) -> etagere::Size {
    etagere::Size::new(size.width.into(), size.height.into())
}

fn point_from_etagere(value: etagere::Point) -> Point<DevicePixels> {
    Point {
        x: DevicePixels::from(value.x),
        y: DevicePixels::from(value.y),
    }
}

#[derive(Deref, DerefMut)]
struct AssertSend<T>(T);

unsafe impl<T> Send for AssertSend<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use foreign_types::ForeignType;
    use gpui::PlatformAtlas;
    use std::borrow::Cow;

    fn create_atlas() -> Option<MetalAtlas> {
        let device = metal::Device::system_default()?;
        Some(MetalAtlas::new(device, true))
    }

    fn make_image_key(image_id: usize, frame_index: usize) -> AtlasKey {
        AtlasKey::Image(gpui::RenderImageParams {
            image_id: gpui::ImageId(image_id),
            frame_index,
        })
    }

    fn insert_tile(atlas: &MetalAtlas, key: &AtlasKey, size: Size<DevicePixels>) -> AtlasTile {
        atlas
            .get_or_insert_with(key, &mut || {
                let byte_count = (size.width.0 as usize) * (size.height.0 as usize) * 4;
                Ok(Some((size, Cow::Owned(vec![0u8; byte_count]))))
            })
            .expect("allocation should succeed")
            .expect("callback returns Some")
    }

    #[test]
    fn test_remove_clears_stale_keys_from_tiles_by_key() {
        let Some(atlas) = create_atlas() else {
            return;
        };

        let small = Size {
            width: DevicePixels(64),
            height: DevicePixels(64),
        };

        let key_a = make_image_key(1, 0);
        let key_b = make_image_key(2, 0);
        let key_c = make_image_key(3, 0);

        let tile_a = insert_tile(&atlas, &key_a, small);
        let tile_b = insert_tile(&atlas, &key_b, small);
        let tile_c = insert_tile(&atlas, &key_c, small);

        assert_eq!(tile_a.texture_id, tile_b.texture_id);
        assert_eq!(tile_b.texture_id, tile_c.texture_id);

        // Remove A: texture still has B and C, so it stays.
        // The key for A must be removed from tiles_by_key.
        atlas.remove(&key_a);

        // Remove B: texture still has C.
        atlas.remove(&key_b);

        // Remove C: texture becomes unreferenced and is deleted.
        atlas.remove(&key_c);

        // Re-inserting A must allocate a fresh tile on a new texture,
        // NOT return a stale tile referencing the deleted texture.
        let tile_a2 = insert_tile(&atlas, &key_a, small);

        // The texture must actually exist — this would panic before the fix.
        let _texture = atlas.metal_texture(tile_a2.texture_id);
    }

    #[test]
    fn test_remove_deallocates_tile_space_for_reuse() {
        let Some(atlas) = create_atlas() else {
            return;
        };

        let small = Size {
            width: DevicePixels(64),
            height: DevicePixels(64),
        };
        let big = Size {
            width: DevicePixels(700),
            height: DevicePixels(700),
        };

        let keeper_key = make_image_key(1, 0);
        let big_key_a = make_image_key(2, 0);
        let big_key_b = make_image_key(3, 0);

        let keeper_tile = insert_tile(&atlas, &keeper_key, small);
        let tile_a = insert_tile(&atlas, &big_key_a, big);
        assert_eq!(keeper_tile.texture_id, tile_a.texture_id);

        atlas.remove(&big_key_a);
        let tile_b = insert_tile(&atlas, &big_key_b, big);
        assert_eq!(tile_b.texture_id, keeper_tile.texture_id);
    }

    #[test]
    fn test_remove_nonexistent_key_is_noop() {
        let Some(atlas) = create_atlas() else {
            return;
        };
        let key = make_image_key(999, 0);
        atlas.remove(&key);
    }

    #[test]
    fn small_atlas_grows_without_moving_tiles_or_losing_pixels() {
        let Some(atlas) = create_atlas() else {
            return;
        };
        let key = make_image_key(1, 0);
        let small = Size {
            width: DevicePixels(32),
            height: DevicePixels(32),
        };
        let pixels = vec![173u8; 32 * 32 * 4];
        let tile = atlas
            .get_or_insert_with(&key, &mut || {
                Ok(Some((small, Cow::Borrowed(pixels.as_slice()))))
            })
            .unwrap()
            .unwrap();
        let original = atlas.metal_texture(tile.texture_id);
        assert_eq!((original.width(), original.height()), (256, 256));
        let larger = insert_tile(
            &atlas,
            &make_image_key(2, 0),
            Size {
                width: DevicePixels(700),
                height: DevicePixels(700),
            },
        );
        assert_eq!(larger.texture_id, tile.texture_id);
        let grown = atlas.metal_texture(tile.texture_id);
        assert_eq!((grown.width(), grown.height()), (1024, 1024));
        assert_ne!(grown.as_ptr(), original.as_ptr());
        assert_eq!(atlas.0.lock().tiles_by_key[&key], tile);
        let mut actual = vec![0u8; pixels.len()];
        let region = metal::MTLRegion::new_2d(
            tile.bounds.origin.x.0 as u64,
            tile.bounds.origin.y.0 as u64,
            32,
            32,
        );
        for texture in [&grown, &original] {
            texture.get_bytes(actual.as_mut_ptr().cast(), 32 * 4, region, 0);
            assert_eq!(
                actual, pixels,
                "both the new atlas and retained old texture preserve artwork"
            );
        }
    }
}
