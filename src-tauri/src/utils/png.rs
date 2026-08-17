//! 极简 PNG 解码器：仅用于截图后的自动质检（黑屏/白屏/纯色屏检测）。
//!
//! 不引入图像库，自行解析 PNG 的 IHDR + IDAT + zlib（stored/fixed/dynamic Huffman），
//! 支持 8-bit RGB/RGBA，缩放读取以控制内存。覆盖鸿蒙设备截图的常见格式。

/// 解码后的位图（RGB，已去除 alpha 并做下采样）。
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

// ---------- 最小 PNG 编码（纯色图标生成，供工程创建模板写 PNG 资源） ----------

/// 生成一张纯色 PNG（8-bit RGB，zlib stored 块），用于创建工程时的图标占位。
pub fn encode_solid_png(width: u32, height: u32, color: &[u8; 3]) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err(format!("非法尺寸：{width}x{height}（支持 1-4096）"));
    }
    let data: Vec<u8> = (0..width * height).flat_map(|_| color.iter().copied()).collect();
    // 每行 1 字节 filter（Sub=1）+ 行数据：纯色行对 Sub 滤波后全零
    let mut raw = Vec::with_capacity(((width * 3 + 1) * height) as usize);
    for y in 0..height {
        let row = &data[(y * width * 3) as usize..((y + 1) * width * 3) as usize];
        let mut filtered = Vec::with_capacity(row.len() + 1);
        filtered.push(1u8); // Sub filter
        for (i, &b) in row.iter().enumerate() {
            let left = if i >= 3 { row[i - 3] } else { 0 };
            filtered.push(b.wrapping_sub(left));
        }
        raw.extend_from_slice(&filtered);
    }
    let comp = zlib_stored(&raw);
    let mut png = vec![137, 80, 78, 71, 13, 10, 26, 10];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
    png.extend_from_slice(&make_chunk(b"IHDR", &ihdr));
    png.extend_from_slice(&make_chunk(b"IDAT", &comp));
    png.extend_from_slice(&make_chunk(b"IEND", &[]));
    Ok(png)
}

fn make_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = Vec::new();
    c.extend_from_slice(&(data.len() as u32).to_be_bytes());
    c.extend_from_slice(kind);
    c.extend_from_slice(data);
    let crc = crc32(kind, data);
    c.extend_from_slice(&crc.to_be_bytes());
    c
}

fn crc32(kind: &[u8], data: &[u8]) -> u32 {
    let mut crc = 0xffffffffu32;
    for &b in kind.iter().chain(data.iter()) {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xedb88320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

// zlib 头 + 单个 stored deflate 块 + adler32
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut pos = 0;
    loop {
        let chunk = data.len().saturating_sub(pos).min(65535);
        let last = if pos + chunk >= data.len() { 1u8 } else { 0 };
        out.push(last);
        let len = chunk as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(&data[pos..pos + chunk]);
        pos += chunk;
        if last == 1 {
            break;
        }
    }
    let adler = adler32(data);
    out.extend_from_slice(&adler.to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}


/// 从 PNG 字节解码出下采样后的 RGB 位图。
/// `max_dim` 控制返回图的最大边长（等比缩小），用于降低质检计算量。
pub fn decode_png(data: &[u8], max_dim: u32) -> Result<Image, String> {
    if data.len() < 8 || &data[0..8] != [137, 80, 78, 71, 13, 10, 26, 10] {
        return Err("not a png".into());
    }
    let mut pos = 8;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut bit_depth = 0u8;
    let mut color_type = 0u8;
    let mut idat = Vec::new();
    while pos + 8 <= data.len() {
        let len = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let kind = &data[pos + 4..pos + 8];
        let chunk_start = pos + 8;
        let chunk_end = chunk_start + len;
        if chunk_end > data.len() {
            break;
        }
        match kind {
            b"IHDR" => {
                width = u32::from_be_bytes([data[chunk_start], data[chunk_start + 1], data[chunk_start + 2], data[chunk_start + 3]]);
                height = u32::from_be_bytes([data[chunk_start + 4], data[chunk_start + 5], data[chunk_start + 6], data[chunk_start + 7]]);
                bit_depth = data[chunk_start + 8];
                color_type = data[chunk_start + 9];
            }
            b"IDAT" => idat.extend_from_slice(&data[chunk_start..chunk_end]),
            b"IEND" => break,
            _ => {}
        }
        pos = chunk_end + 4; // 跳过 CRC
    }
    if width == 0 || height == 0 {
        return Err("invalid ihdr".into());
    }
    if bit_depth != 8 {
        return Err(format!("unsupported bit depth {bit_depth}"));
    }
    let channels = match color_type {
        2 => 3usize, // RGB
        6 => 4,      // RGBA
        _ => return Err(format!("unsupported color type {color_type}")),
    };

    let raw = inflate(&idat).map_err(|e| format!("inflate: {e}"))?;
    let bpp = channels;
    let stride = width as usize * bpp;
    let mut rows: Vec<&[u8]> = Vec::with_capacity(height as usize);
    let mut off = 0usize;
    for _ in 0..height {
        let end = off + 1 + stride;
        if end > raw.len() {
            return Err("truncated image data".into());
        }
        rows.push(&raw[off..end]);
        off = end;
    }

    // 下采样步长
    let step = ((width.max(height)) / max_dim).max(1) as usize;
    let out_w = (width as usize / step).max(1);
    let out_h = (height as usize / step).max(1);
    let mut rgb = vec![0u8; (out_w * out_h * 3) as usize];
    let mut prev: Vec<u8> = vec![0; stride];
    for (y, row) in rows.iter().enumerate() {
        let filter = row[0];
        let src = &row[1..];
        let mut recon = vec![0u8; stride];
        for x in 0..stride {
            let left = if x >= bpp { recon[x - bpp] } else { 0 };
            let up = prev[x];
            let upleft = if x >= bpp { prev[x - bpp] } else { 0 };
            let v = src[x] as i16;
            recon[x] = match filter {
                0 => v as u8,
                1 => (v + left as i16) as u8,
                2 => (v + up as i16) as u8,
                3 => (v + ((left as i16 + up as i16) / 2)) as u8,
                4 => {
                    let p = left as i16 + up as i16 - upleft as i16;
                    let pa = (p - left as i16).abs();
                    let pb = (p - up as i16).abs();
                    let pc = (p - upleft as i16).abs();
                    let pr = if pa <= pb && pa <= pc { left } else if pb <= pc { up } else { upleft };
                    (v + pr as i16) as u8
                }
                _ => return Err("bad filter".into()),
            };
        }
        if y % step == 0 {
            for sx in 0..out_w {
                let ix = sx * step;
                let dst = ((y / step) * out_w + sx) * 3;
                rgb[dst] = recon[ix * bpp];
                rgb[dst + 1] = recon[ix * bpp + 1];
                rgb[dst + 2] = recon[ix * bpp + 2];
            }
        }
        prev = recon;
    }
    Ok(Image { width: out_w as u32, height: out_h as u32, rgb })
}

/// 屏幕质检结果。
pub struct ScreenCheck {
    /// 平均亮度 0-255
    pub avg_brightness: f64,
    /// 像素间颜色差异的平均值（越低越接近纯色/黑屏）
    pub variance: f64,
    /// 判定：黑屏
    pub is_black: bool,
    /// 判定：白屏/过曝
    pub is_white: bool,
    /// 判定：异常纯色（差异极低且非黑非白，可能卡在启动页/渲染失败）
    pub is_flat: bool,
}

pub fn analyze(img: &Image) -> ScreenCheck {
    let n = img.rgb.len() as f64 / 3.0;
    let mut sum = 0u64;
    for rgb in img.rgb.chunks(3) {
        sum += ((rgb[0] as u64 + rgb[1] as u64 + rgb[2] as u64) / 3) as u64;
    }
    let avg = sum as f64 / n;
    let mut var = 0u64;
    for rgb in img.rgb.chunks(3) {
        let b = (rgb[0] as i32 + rgb[1] as i32 + rgb[2] as i32) / 3;
        let d = (b as f64 - avg).abs();
        var += d as u64;
    }
    let variance = var as f64 / n;
    let is_black = avg < 12.0;
    let is_white = avg > 245.0;
    let is_flat = variance < 6.0 && !is_black && !is_white;
    ScreenCheck { avg_brightness: avg, variance, is_black, is_white, is_flat }
}

// ---------- 最小 zlib inflate（RFC 1951） ----------

fn inflate(input: &[u8]) -> Result<Vec<u8>, String> {
    // zlib 封装：2 字节头（CMF/FLG，如 0x78 0x01）+ deflate 数据 + 4 字节 adler32 尾部。
    // 必须先跳过 zlib 头，否则把 CMF/FLG 当 deflate 块头解析会全部错位。
    if input.len() < 6 {
        return Err("zlib data too short".into());
    }
    let cmf = input[0] as u16;
    let flg = input[1] as u16;
    if (cmf * 256 + flg) % 31 != 0 {
        return Err("bad zlib header".into());
    }
    let mut out = Vec::new();
    let mut s = BitStream { data: input, pos: 2, bit: 0 };
    loop {
        let bfinal = s.read_bits(1)?;
        let btype = s.read_bits(2)?;
        match btype {
            0 => s.copy_stored(&mut out)?,
            1 => s.inflate_fixed(&mut out)?,
            2 => s.inflate_dynamic(&mut out)?,
            _ => return Err("invalid block type".into()),
        }
        if bfinal == 1 {
            break;
        }
    }
    Ok(out)
}

struct BitStream<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u32,
}

impl<'a> BitStream<'a> {
    fn read_bits(&mut self, n: u32) -> Result<u32, String> {
        let mut val = 0u32;
        for i in 0..n {
            if self.pos >= self.data.len() {
                return Err("eof".into());
            }
            let b = ((self.data[self.pos] >> self.bit) & 1) as u32;
            val |= b << i;
            self.bit += 1;
            if self.bit == 8 {
                self.bit = 0;
                self.pos += 1;
            }
        }
        Ok(val)
    }
    fn align_byte(&mut self) {
        if self.bit != 0 {
            self.bit = 0;
            self.pos += 1;
        }
    }
    fn copy_stored(&mut self, out: &mut Vec<u8>) -> Result<(), String> {
        self.align_byte();
        if self.pos + 4 > self.data.len() {
            return Err("eof stored".into());
        }
        let len = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]) as usize;
        self.pos += 4;
        if self.pos + len > self.data.len() {
            return Err("stored overrun".into());
        }
        out.extend_from_slice(&self.data[self.pos..self.pos + len]);
        self.pos += len;
        Ok(())
    }
    fn build_fixed_tables(&self) -> (HuffTable, HuffTable) {
        let mut lit = vec![0u8; 288];
        for (i, v) in lit.iter_mut().enumerate() {
            *v = match i {
                0..=143 => 8,
                144..=255 => 9,
                256..=279 => 7,
                _ => 8,
            };
        }
        let dist = vec![5u8; 30];
        (HuffTable::build(&lit).unwrap(), HuffTable::build(&dist).unwrap())
    }
    fn inflate_fixed(&mut self, out: &mut Vec<u8>) -> Result<(), String> {
        let (lit, dist) = self.build_fixed_tables();
        self.decode_block(&lit, &dist, out)
    }
    fn inflate_dynamic(&mut self, out: &mut Vec<u8>) -> Result<(), String> {
        let hlit = self.read_bits(5)? as usize + 257;
        let hdist = self.read_bits(5)? as usize + 1;
        let hclen = self.read_bits(4)? as usize + 4;
        let order = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
        let mut cl_len = vec![0u8; 19];
        for i in 0..hclen {
            cl_len[order[i]] = self.read_bits(3)? as u8;
        }
        let cl_table = HuffTable::build(&cl_len).map_err(|e| format!("cl: {e}"))?;
        let mut code_lens = Vec::with_capacity(hlit + hdist);
        while code_lens.len() < hlit + hdist {
            let sym = self.decode_sym(&cl_table)?;
            match sym {
                16 => {
                    let rep = self.read_bits(2)? as usize + 3;
                    let last = *code_lens.last().ok_or("repeat underrun")?;
                    for _ in 0..rep {
                        code_lens.push(last);
                    }
                }
                17 => {
                    let rep = self.read_bits(3)? as usize + 3;
                    for _ in 0..rep {
                        code_lens.push(0);
                    }
                }
                18 => {
                    let rep = self.read_bits(7)? as usize + 11;
                    for _ in 0..rep {
                        code_lens.push(0);
                    }
                }
                _ => code_lens.push(sym as u8),
            }
        }
        let lit_table = HuffTable::build(&code_lens[..hlit]).map_err(|e| format!("lit: {e}"))?;
        let dist_table = HuffTable::build(&code_lens[hlit..]).map_err(|e| format!("dist: {e}"))?;
        self.decode_block(&lit_table, &dist_table, out)
    }
    fn decode_sym(&mut self, table: &HuffTable) -> Result<usize, String> {
        let mut code = 0u32;
        for len in 1..=table.max_len {
            code |= self.read_bits(1)?;
            if let Some(&sym) = table.lookup.get(&(code, len)) {
                return Ok(sym);
            }
        }
        Err("huffman code not found".into())
    }
    fn decode_block(&mut self, lit: &HuffTable, dist: &HuffTable, out: &mut Vec<u8>) -> Result<(), String> {
        let len_base = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
        let len_extra = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
        let dist_base = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
        let dist_extra = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
        loop {
            let sym = self.decode_sym(lit)?;
            match sym {
                0..=255 => out.push(sym as u8),
                256 => break,
                257..=285 => {
                    let idx = sym - 257;
                    let mut length = len_base[idx] as usize;
                    if len_extra[idx] > 0 {
                        length += self.read_bits(len_extra[idx])? as usize;
                    }
                    let dsym = self.decode_sym(dist)?;
                    if dsym >= 30 {
                        return Err("bad dist sym".into());
                    }
                    let mut distance = dist_base[dsym] as usize;
                    if dist_extra[dsym] > 0 {
                        distance += self.read_bits(dist_extra[dsym])? as usize;
                    }
                    if distance > out.len() {
                        return Err("dist too far".into());
                    }
                    let start = out.len() - distance;
                    for i in 0..length {
                        let b = out[start + (i % distance)];
                        out.push(b);
                    }
                }
                _ => return Err("bad lit sym".into()),
            }
        }
        Ok(())
    }
}

struct HuffTable {
    lookup: std::collections::HashMap<(u32, u8), usize>,
    max_len: u8,
}

impl HuffTable {
    fn build(lengths: &[u8]) -> Result<Self, String> {
        let max_len = *lengths.iter().max().unwrap_or(&0);
        if max_len == 0 {
            return Ok(HuffTable { lookup: std::collections::HashMap::new(), max_len: 0 });
        }
        let mut bl_count = vec![0u32; max_len as usize + 1];
        for &l in lengths {
            if l > 0 {
                bl_count[l as usize] += 1;
            }
        }
        let mut code = 0u32;
        let mut next_code = vec![0u32; max_len as usize + 1];
        for bits in 1..=max_len as usize {
            code = (code + bl_count[bits - 1]) << 1;
            next_code[bits] = code;
        }
        let mut lookup = std::collections::HashMap::new();
        for (sym, &len) in lengths.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let c = next_code[len as usize];
            let rev = reverse_bits(c, len);
            next_code[len as usize] += 1;
            lookup.insert((rev, len), sym);
        }
        Ok(HuffTable { lookup, max_len })
    }
}

fn reverse_bits(mut code: u32, len: u8) -> u32 {
    let mut out = 0u32;
    for _ in 0..len {
        out = (out << 1) | (code & 1);
        code >>= 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_black_screen() {
        // 4x4 纯黑 RGB PNG
        let png = make_png(4, 4, &[0, 0, 0]);
        let img = decode_png(&png, 64).unwrap();
        let c = analyze(&img);
        assert!(c.is_black, "avg={} var={}", c.avg_brightness, c.variance);
    }

    #[test]
    fn detects_white_screen() {
        let png = make_png(4, 4, &[255, 255, 255]);
        let img = decode_png(&png, 64).unwrap();
        let c = analyze(&img);
        assert!(c.is_white);
    }

    #[test]
    fn detects_flat_color_screen() {
        let png = make_png(8, 8, &[200, 30, 30]);
        let img = decode_png(&png, 64).unwrap();
        let c = analyze(&img);
        assert!(c.is_flat, "var={}", c.variance);
    }

    #[test]
    fn varied_screen_not_flagged() {
        let mut data = Vec::new();
        for y in 0..8 {
            for x in 0..8 {
                let v = ((x + y) * 16) as u8;
                data.extend_from_slice(&[v, v, 255 - v]);
            }
        }
        let png = make_png_rgb(8, 8, &data);
        let img = decode_png(&png, 64).unwrap();
        let c = analyze(&img);
        assert!(!c.is_black && !c.is_white && !c.is_flat, "avg={} var={}", c.avg_brightness, c.variance);
    }

    fn make_png(w: u32, h: u32, color: &[u8; 3]) -> Vec<u8> {
        encode_solid_png(w, h, color).expect("编码应成功")
    }

    fn make_png_rgb(w: u32, h: u32, rgb: &[u8]) -> Vec<u8> {
        let mut raw = Vec::with_capacity(((w * 3 + 1) * h) as usize);
        for y in 0..h {
            raw.push(0u8);
            raw.extend_from_slice(&rgb[(y * w * 3) as usize..((y + 1) * w * 3) as usize]);
        }
        // 用 zlib 格式（0x78 0x01 固定 Huffman 即可）——这里直接构造 stored 块更可控
        let comp = zlib_stored(&raw);
        let mut png = vec![137, 80, 78, 71, 13, 10, 26, 10];
        let ihdr = make_chunk(b"IHDR", &{
            let mut v = Vec::new();
            v.extend_from_slice(&w.to_be_bytes());
            v.extend_from_slice(&h.to_be_bytes());
            v.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
            v
        });
        png.extend_from_slice(&ihdr);
        png.extend_from_slice(&make_chunk(b"IDAT", &comp));
        png.extend_from_slice(&make_chunk(b"IEND", &[]));
        png
    }

    #[test]
    fn encode_solid_roundtrip() {
        // 编码出的纯色 PNG 应能被本模块解码器正确还原
        let png = encode_solid_png(32, 32, &[12, 34, 56]).expect("编码成功");
        let img = decode_png(&png, 64).expect("解码成功");
        assert_eq!(img.width, 32);
        assert_eq!(img.height, 32);
        assert!(img.rgb.iter().all(|&b| b == 12 || b == 34 || b == 56));
        assert_eq!(&img.rgb[0..3], &[12, 34, 56]);
    }
}
