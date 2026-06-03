use std::collections::BTreeMap;
use std::io::Cursor;

use wmv_decoder::{AsfWmv2Decoder, YuvFrame};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct MSpriteHandle(pub u32);

#[derive(Debug)]
pub struct MSpriteSystem {
    next_handle: u32,
    entries: BTreeMap<MSpriteHandle, MSpriteEntry>,
    movie: Option<MoviePlayback>,
}

impl Default for MSpriteSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl MSpriteSystem {
    pub fn new() -> Self {
        Self {
            next_handle: 1,
            entries: BTreeMap::new(),
            movie: None,
        }
    }

    pub fn load_wmv(
        &mut self,
        name: impl Into<String>,
        bytes: Vec<u8>,
    ) -> anyhow::Result<LoadedMSprite> {
        let name = name.into();
        let mut decoder = AsfWmv2Decoder::open(Cursor::new(bytes.clone()))?;
        let info = decoder.video_stream_info().clone();
        let first = decoder.next_frame()?;
        let (width, height, rgba, pts_ms) = match first {
            Some(frame) => {
                let rgba = yuv420_to_rgba(&frame.frame);
                (frame.frame.width, frame.frame.height, rgba, frame.pts_ms)
            }
            None => {
                let rgba = vec![0; info.width.max(1) as usize * info.height.max(1) as usize * 4];
                (info.width.max(1), info.height.max(1), rgba, 0)
            }
        };

        let handle = self.allocate_handle();
        self.entries.insert(
            handle,
            MSpriteEntry {
                name: name.clone(),
                bytes,
                decoder,
                width,
                height,
                current_rgba: rgba.clone(),
                current_pts_ms: pts_ms,
                playing: false,
                locked: false,
                loop_mode: 0,
                loop_start: 0,
                loop_end: 0,
                finished: false,
                state_bits: 0,
            },
        );

        Ok(LoadedMSprite {
            handle,
            width,
            height,
            rgba,
            name,
        })
    }

    pub fn release(&mut self, handle: MSpriteHandle) -> bool {
        self.entries.remove(&handle).is_some()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn check(&self, handle: MSpriteHandle) -> bool {
        self.entries.contains_key(&handle)
    }

    pub fn play(&mut self, handle: MSpriteHandle, loop_mode: i32) -> bool {
        let Some(entry) = self.entries.get_mut(&handle) else {
            return false;
        };
        entry.playing = true;
        entry.finished = false;
        entry.loop_mode = loop_mode;
        entry.state_bits &= !MSPRITE_STATE_FINISHED;
        true
    }

    pub fn stop(&mut self, handle: MSpriteHandle) -> bool {
        let Some(entry) = self.entries.get_mut(&handle) else {
            return false;
        };
        entry.playing = false;
        entry.finished = true;
        entry.state_bits |= MSPRITE_STATE_FINISHED;
        true
    }

    pub fn pause(&mut self, handle: MSpriteHandle) -> bool {
        let Some(entry) = self.entries.get_mut(&handle) else {
            return false;
        };
        entry.playing = false;
        true
    }

    pub fn lock(&mut self, handle: MSpriteHandle) -> bool {
        let Some(entry) = self.entries.get_mut(&handle) else {
            return false;
        };
        entry.locked = true;
        true
    }

    pub fn unlock(&mut self, handle: MSpriteHandle) -> bool {
        let Some(entry) = self.entries.get_mut(&handle) else {
            return false;
        };
        entry.locked = false;
        true
    }

    pub fn set_loop(&mut self, handle: MSpriteHandle, loop_mode: i32) -> bool {
        let Some(entry) = self.entries.get_mut(&handle) else {
            return false;
        };
        entry.loop_mode = loop_mode;
        true
    }

    pub fn set_loop_point(&mut self, handle: MSpriteHandle, start: i32, end: i32) -> bool {
        let Some(entry) = self.entries.get_mut(&handle) else {
            return false;
        };
        entry.loop_start = start.max(0);
        entry.loop_end = end.max(0);
        true
    }

    pub fn is_loop(&self, handle: MSpriteHandle) -> bool {
        self.entries
            .get(&handle)
            .is_some_and(|entry| entry.loop_mode != 0 || entry.loop_end > entry.loop_start)
    }

    pub fn state(&self, handle: MSpriteHandle) -> u32 {
        let Some(entry) = self.entries.get(&handle) else {
            return 0;
        };
        let mut state = entry.state_bits;
        if entry.playing {
            state |= MSPRITE_STATE_PLAYING;
        }
        if entry.finished {
            state |= MSPRITE_STATE_FINISHED;
        }
        if entry.locked {
            state |= MSPRITE_STATE_LOCKED;
        }
        if self.is_loop(handle) {
            state |= MSPRITE_STATE_LOOP;
        }
        state
    }

    pub fn advance(&mut self, delta_ms: u32) -> Vec<MSpriteFrameUpdate> {
        let mut updates = Vec::new();
        let handles = self.entries.keys().copied().collect::<Vec<_>>();
        for handle in handles {
            let Some(entry) = self.entries.get_mut(&handle) else {
                continue;
            };
            if !entry.playing || entry.locked || entry.finished {
                continue;
            }
            let target_pts = entry.current_pts_ms.saturating_add(delta_ms);
            let mut latest = None;
            loop {
                match entry.decoder.next_frame() {
                    Ok(Some(frame)) => {
                        let pts = frame.pts_ms;
                        let rgba = yuv420_to_rgba(&frame.frame);
                        entry.width = frame.frame.width;
                        entry.height = frame.frame.height;
                        entry.current_pts_ms = pts;
                        entry.current_rgba = rgba.clone();
                        latest = Some(MSpriteFrameUpdate {
                            handle,
                            width: frame.frame.width,
                            height: frame.frame.height,
                            rgba,
                            source_name: entry.name.clone(),
                        });
                        if pts >= target_pts {
                            break;
                        }
                    }
                    Ok(None) => {
                        if entry.loop_mode != 0 {
                            match AsfWmv2Decoder::open(Cursor::new(entry.bytes.clone())) {
                                Ok(decoder) => {
                                    entry.decoder = decoder;
                                    entry.current_pts_ms = entry.loop_start.max(0) as u32;
                                    continue;
                                }
                                Err(err) => {
                                    log::warn!(
                                        "[trace-msprite] restart {:?} failed: {err}",
                                        entry.name
                                    );
                                }
                            }
                        }
                        entry.playing = false;
                        entry.finished = true;
                        entry.state_bits |= MSPRITE_STATE_FINISHED;
                        break;
                    }
                    Err(err) => {
                        log::warn!("[trace-msprite] decode {:?} failed: {err}", entry.name);
                        entry.playing = false;
                        entry.finished = true;
                        entry.state_bits |= MSPRITE_STATE_FINISHED;
                        break;
                    }
                }
            }
            if let Some(update) = latest {
                updates.push(update);
            }
        }
        updates
    }

    pub fn start_movie(&mut self, name: impl Into<String>, layer: i32) {
        self.movie = Some(MoviePlayback {
            name: name.into(),
            layer,
            playing: true,
            elapsed_ms: 0,
        });
    }

    pub fn stop_movie(&mut self) {
        self.movie = None;
    }

    pub fn is_movie(&self) -> bool {
        self.movie.as_ref().is_some_and(|movie| movie.playing)
    }

    pub fn movie(&self) -> Option<&MoviePlayback> {
        self.movie.as_ref()
    }

    fn allocate_handle(&mut self) -> MSpriteHandle {
        let handle = MSpriteHandle(self.next_handle);
        self.next_handle = self
            .next_handle
            .checked_add(1)
            .expect("MSprite handle space exhausted");
        handle
    }
}

#[derive(Clone, Debug)]
pub struct LoadedMSprite {
    pub handle: MSpriteHandle,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct MSpriteFrameUpdate {
    pub handle: MSpriteHandle,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub source_name: String,
}

#[derive(Clone, Debug)]
pub struct MoviePlayback {
    pub name: String,
    pub layer: i32,
    pub playing: bool,
    pub elapsed_ms: u32,
}

struct MSpriteEntry {
    name: String,
    bytes: Vec<u8>,
    decoder: AsfWmv2Decoder<Cursor<Vec<u8>>>,
    width: u32,
    height: u32,
    current_rgba: Vec<u8>,
    current_pts_ms: u32,
    playing: bool,
    locked: bool,
    loop_mode: i32,
    loop_start: i32,
    loop_end: i32,
    finished: bool,
    state_bits: u32,
}

impl std::fmt::Debug for MSpriteEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MSpriteEntry")
            .field("name", &self.name)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("current_pts_ms", &self.current_pts_ms)
            .field("playing", &self.playing)
            .field("locked", &self.locked)
            .field("loop_mode", &self.loop_mode)
            .field("loop_start", &self.loop_start)
            .field("loop_end", &self.loop_end)
            .field("finished", &self.finished)
            .field("state_bits", &self.state_bits)
            .finish_non_exhaustive()
    }
}

pub const MSPRITE_STATE_PLAYING: u32 = 0x0000_0001;
pub const MSPRITE_STATE_LOCKED: u32 = 0x0000_0002;
pub const MSPRITE_STATE_FINISHED: u32 = 0x0000_0004;
pub const MSPRITE_STATE_LOOP: u32 = 0x8000_0000;

fn yuv420_to_rgba(frame: &YuvFrame) -> Vec<u8> {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let chroma_width = (width / 2).max(1);
    let mut rgba = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let yy = frame.y[y * width + x] as i32;
            let uv_idx = (y / 2) * chroma_width + (x / 2);
            let cb = frame.cb.get(uv_idx).copied().unwrap_or(128) as i32;
            let cr = frame.cr.get(uv_idx).copied().unwrap_or(128) as i32;
            let c = yy - 16;
            let d = cb - 128;
            let e = cr - 128;
            let r = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
            let g = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
            let b = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
            let out = (y * width + x) * 4;
            rgba[out] = r;
            rgba[out + 1] = g;
            rgba[out + 2] = b;
            rgba[out + 3] = 255;
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yuv420_black_frame_converts_to_opaque_rgba() {
        let frame = YuvFrame::new(2, 2);
        let rgba = yuv420_to_rgba(&frame);
        assert_eq!(rgba.len(), 16);
        assert!(rgba.chunks_exact(4).all(|px| px[3] == 255));
        assert!(rgba
            .chunks_exact(4)
            .all(|px| px[0] <= 1 && px[1] <= 1 && px[2] <= 1));
    }
}
