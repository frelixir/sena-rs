#[derive(Clone, Debug)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub offset_x: i32,
    pub offset_y: i32,
    pub rgba: Vec<u8>,
}

pub fn decode_image(bytes: &[u8]) -> anyhow::Result<DecodedImage> {
    if bytes.len() >= 0x20 && bytes.get(0..3) == Some(b"GE ") {
        return decode_pgd_ge(bytes);
    }
    if bytes.len() >= 0x38 && bytes.get(0..4) == Some(b"PGD3") {
        return decode_pgd3(bytes);
    }
    anyhow::bail!("unsupported image signature");
}

fn decode_pgd3(bytes: &[u8]) -> anyhow::Result<DecodedImage> {
    let offset_x = read_u16(bytes, 0x04)? as i16 as i32;
    let offset_y = read_u16(bytes, 0x06)? as i16 as i32;
    let width = read_u16(bytes, 0x08)? as usize;
    let height = read_u16(bytes, 0x0A)? as usize;
    let bpp = read_u16(bytes, 0x0C)?;
    let unpacked_size = read_u32(bytes, 0x30)? as usize;
    let packed_size = read_u32(bytes, 0x34)? as usize;
    let data_end = 0x38usize
        .checked_add(packed_size)
        .ok_or_else(|| anyhow::anyhow!("PGD3 packed stream size overflow"))?;
    if data_end > bytes.len() {
        anyhow::bail!(
            "PGD3 packed stream out of bounds: end {data_end}, len {}",
            bytes.len()
        );
    }

    let mut input = Cursor::new(&bytes[..data_end], 0x38);
    let unpacked = unpack_ge_pre(&mut input, unpacked_size)?;
    let pixel_size = match bpp {
        24 => 3,
        32 => 4,
        _ => anyhow::bail!("unsupported PGD3 bpp {bpp}"),
    };
    let bgx = post_process_pal(&unpacked, 0, width, height, pixel_size)?;
    let rgba = bgx_to_rgba(&bgx, pixel_size)?;
    let expected = width
        .checked_mul(height)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("PGD3 dimensions overflow"))?;
    if rgba.len() != expected {
        anyhow::bail!(
            "PGD3 decoded size mismatch: got {}, expected {}",
            rgba.len(),
            expected
        );
    }

    Ok(DecodedImage {
        width: width as u32,
        height: height as u32,
        cell_width: width.max(1) as u32,
        cell_height: height.max(1) as u32,
        offset_x,
        offset_y,
        rgba,
    })
}

fn decode_pgd_ge(bytes: &[u8]) -> anyhow::Result<DecodedImage> {
    let offset_x = read_u32(bytes, 0x04)? as i32;
    let offset_y = read_u32(bytes, 0x08)? as i32;
    let mut width = read_u32(bytes, 0x0C)? as usize;
    let mut height = read_u32(bytes, 0x10)? as usize;
    let cell_width = read_u32(bytes, 0x14)?.max(1);
    let cell_height = read_u32(bytes, 0x18)?.max(1);
    let method = read_u16(bytes, 0x1C)?;
    let unpacked_size = read_u32(bytes, 0x20)? as usize;
    let mut input = Cursor::new(bytes, 0x28);
    let unpacked = unpack_ge_pre(&mut input, unpacked_size)?;
    let rgba = match method {
        1 => post_process_planes_bgra_to_rgba(&unpacked)?,
        2 => post_process_yuv_like_to_rgba(&unpacked, width, height)?,
        3 => {
            let bpp = read_u16(&unpacked, 2)?;
            width = read_u16(&unpacked, 4)? as usize;
            height = read_u16(&unpacked, 6)? as usize;
            let pixel_size = match bpp {
                24 => 3,
                32 => 4,
                _ => anyhow::bail!("unsupported PGD/GE bpp {bpp}"),
            };
            let bgx = post_process_pal(&unpacked, 8, width, height, pixel_size)?;
            bgx_to_rgba(&bgx, pixel_size)?
        }
        _ => anyhow::bail!("unsupported PGD/GE method {method}"),
    };
    let expected = width
        .checked_mul(height)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("PGD/GE dimensions overflow"))?;
    if rgba.len() != expected {
        anyhow::bail!(
            "PGD/GE decoded size mismatch: got {}, expected {}",
            rgba.len(),
            expected
        );
    }
    Ok(DecodedImage {
        width: width as u32,
        height: height as u32,
        cell_width: cell_width.min(width as u32).max(1),
        cell_height: cell_height.min(height as u32).max(1),
        offset_x,
        offset_y,
        rgba,
    })
}

fn unpack_ge_pre(input: &mut Cursor<'_>, unpacked_size: usize) -> anyhow::Result<Vec<u8>> {
    let mut output = vec![0u8; unpacked_size];
    let mut dst = 0usize;
    let mut ctl = 2u16;
    while dst < output.len() {
        ctl >>= 1;
        if ctl == 1 {
            ctl = u16::from(input.read_u8()?) | 0x100;
        }
        let count = if (ctl & 1) != 0 {
            let raw = input.read_u16()? as usize;
            let mut count = raw & 7;
            if (raw & 8) == 0 {
                count = (count << 8) | usize::from(input.read_u8()?);
            }
            count += 4;
            let offset = raw >> 4;
            if offset == 0 || offset > dst {
                anyhow::bail!("invalid PGD/GE back-reference offset {offset} at {dst}");
            }
            copy_overlapped(&mut output, dst - offset, dst, count)?;
            count
        } else {
            let count = usize::from(input.read_u8()?);
            input.read_exact(&mut output[dst..dst + count])?;
            count
        };
        dst = dst.saturating_add(count);
    }
    Ok(output)
}

fn post_process_planes_bgra_to_rgba(input: &[u8]) -> anyhow::Result<Vec<u8>> {
    if input.len() % 4 != 0 {
        anyhow::bail!("PGD/GE method 1 payload is not four planes");
    }
    let plane = input.len() / 4;
    let (a, rest) = input.split_at(plane);
    let (r, rest) = rest.split_at(plane);
    let (g, b) = rest.split_at(plane);
    let mut output = Vec::with_capacity(input.len());
    for i in 0..plane {
        output.extend_from_slice(&[r[i], g[i], b[i], a[i]]);
    }
    Ok(output)
}

fn post_process_yuv_like_to_rgba(
    input: &[u8],
    width: usize,
    height: usize,
) -> anyhow::Result<Vec<u8>> {
    if width % 2 != 0 || height % 2 != 0 {
        anyhow::bail!("PGD/GE method 2 dimensions must be even");
    }
    let segment = width
        .checked_mul(height)
        .ok_or_else(|| anyhow::anyhow!("PGD/GE dimensions overflow"))?
        / 4;
    if input.len() < segment * 3 {
        anyhow::bail!("PGD/GE method 2 payload too short");
    }
    let mut src0 = 0usize;
    let mut src1 = segment;
    let mut src2 = segment * 2;
    let stride = width * 4;
    let mut output = vec![0u8; stride * height];
    let points = [0usize, 1, width, width + 1];
    for y in 0..height / 2 {
        for x in 0..width / 2 {
            let i0 = input[src0] as i8 as i32;
            let i1 = input[src1] as i8 as i32;
            let b_delta = 226 * i0;
            let g_delta = -43 * i0 - 89 * i1;
            let r_delta = 179 * i1;
            src0 += 1;
            src1 += 1;
            for point in points {
                let base = i32::from(input[src2 + point]) << 7;
                let px = ((y * 2) * width + (x * 2) + point) * 4;
                output[px] = clamp_u8((base + r_delta) >> 7);
                output[px + 1] = clamp_u8((base + g_delta) >> 7);
                output[px + 2] = clamp_u8((base + b_delta) >> 7);
                output[px + 3] = 255;
            }
            src2 += 2;
        }
        src2 += width;
    }
    Ok(output)
}

fn post_process_pal(
    input: &[u8],
    mut src: usize,
    width: usize,
    height: usize,
    pixel_size: usize,
) -> anyhow::Result<Vec<u8>> {
    let stride = width
        .checked_mul(pixel_size)
        .ok_or_else(|| anyhow::anyhow!("PGD/GE stride overflow"))?;
    let mut output = vec![0u8; height * stride];
    let mut ctl = src;
    src += height;
    let mut dst = 0usize;
    for row in 0..height {
        let c = *input
            .get(ctl)
            .ok_or_else(|| anyhow::anyhow!("PGD/GE row control out of bounds"))?;
        ctl += 1;
        if (c & 1) != 0 {
            copy_input(input, src, &mut output, dst, pixel_size)?;
            src += pixel_size;
            let mut prev = dst;
            dst += pixel_size;
            for _ in 0..stride - pixel_size {
                let delta = *input
                    .get(src)
                    .ok_or_else(|| anyhow::anyhow!("PGD/GE row data out of bounds"))?;
                output[dst] = output[prev].wrapping_sub(delta);
                src += 1;
                dst += 1;
                prev += 1;
            }
        } else if (c & 2) != 0 {
            if row == 0 {
                anyhow::bail!("PGD/GE row references previous row before row 0");
            }
            let mut prev = dst - stride;
            for _ in 0..stride {
                let delta = *input
                    .get(src)
                    .ok_or_else(|| anyhow::anyhow!("PGD/GE row data out of bounds"))?;
                output[dst] = output[prev].wrapping_sub(delta);
                src += 1;
                dst += 1;
                prev += 1;
            }
        } else {
            copy_input(input, src, &mut output, dst, pixel_size)?;
            src += pixel_size;
            dst += pixel_size;
            if row == 0 {
                copy_input(input, src, &mut output, dst, stride - pixel_size)?;
                src += stride - pixel_size;
                dst += stride - pixel_size;
                continue;
            }
            let mut prev = dst - stride;
            for _ in 0..stride - pixel_size {
                let delta = *input
                    .get(src)
                    .ok_or_else(|| anyhow::anyhow!("PGD/GE row data out of bounds"))?;
                output[dst] =
                    ((u16::from(output[prev]) + u16::from(output[dst - pixel_size])) / 2) as u8;
                output[dst] = output[dst].wrapping_sub(delta);
                src += 1;
                dst += 1;
                prev += 1;
            }
        }
    }
    Ok(output)
}

fn bgx_to_rgba(input: &[u8], pixel_size: usize) -> anyhow::Result<Vec<u8>> {
    if input.len() % pixel_size != 0 {
        anyhow::bail!("PGD/GE pixel payload has partial pixel");
    }
    let mut out = Vec::with_capacity(input.len() / pixel_size * 4);
    for px in input.chunks_exact(pixel_size) {
        out.push(px[2]);
        out.push(px[1]);
        out.push(px[0]);
        out.push(if pixel_size == 4 { px[3] } else { 255 });
    }
    Ok(out)
}

fn copy_input(
    input: &[u8],
    src: usize,
    output: &mut [u8],
    dst: usize,
    count: usize,
) -> anyhow::Result<()> {
    let src_end = src
        .checked_add(count)
        .ok_or_else(|| anyhow::anyhow!("PGD/GE source range overflow"))?;
    let dst_end = dst
        .checked_add(count)
        .ok_or_else(|| anyhow::anyhow!("PGD/GE destination range overflow"))?;
    let data = input
        .get(src..src_end)
        .ok_or_else(|| anyhow::anyhow!("PGD/GE source range out of bounds"))?;
    let target = output
        .get_mut(dst..dst_end)
        .ok_or_else(|| anyhow::anyhow!("PGD/GE destination range out of bounds"))?;
    target.copy_from_slice(data);
    Ok(())
}

fn copy_overlapped(output: &mut [u8], src: usize, dst: usize, count: usize) -> anyhow::Result<()> {
    let end = dst
        .checked_add(count)
        .ok_or_else(|| anyhow::anyhow!("PGD/GE overlap copy overflow"))?;
    if end > output.len() {
        anyhow::bail!("PGD/GE overlap copy out of bounds");
    }
    for i in 0..count {
        output[dst + i] = output[src + i];
    }
    Ok(())
}

fn read_u16(bytes: &[u8], offset: usize) -> anyhow::Result<u16> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| anyhow::anyhow!("u16 read out of bounds at 0x{offset:X}"))?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> anyhow::Result<u32> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| anyhow::anyhow!("u32 read out of bounds at 0x{offset:X}"))?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8], pos: usize) -> Self {
        Self { bytes, pos }
    }

    fn read_u8(&mut self) -> anyhow::Result<u8> {
        let byte = *self
            .bytes
            .get(self.pos)
            .ok_or_else(|| anyhow::anyhow!("PGD/GE input out of bounds"))?;
        self.pos += 1;
        Ok(byte)
    }

    fn read_u16(&mut self) -> anyhow::Result<u16> {
        let value = read_u16(self.bytes, self.pos)?;
        self.pos += 2;
        Ok(value)
    }

    fn read_exact(&mut self, dst: &mut [u8]) -> anyhow::Result<()> {
        let end = self
            .pos
            .checked_add(dst.len())
            .ok_or_else(|| anyhow::anyhow!("PGD/GE input range overflow"))?;
        let src = self
            .bytes
            .get(self.pos..end)
            .ok_or_else(|| anyhow::anyhow!("PGD/GE input range out of bounds"))?;
        dst.copy_from_slice(src);
        self.pos = end;
        Ok(())
    }
}
