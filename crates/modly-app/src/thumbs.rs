//! Картинки каталога: логотипы и обложки модов с сайта.
//!
//! Скачивает и кэширует их `modly_core::site::thumb`, а здесь они декодируются и подгоняются под
//! карточку — в фоновом потоке, и в памяти остаются только маленькие копии. Декодеры — те самые
//! функции, которыми resvg (он внутри Slint) читает картинки в SVG: tiny-skia (PNG), zune-jpeg и
//! image-webp, так что в exe они уже есть и почти ничего не добавляют.
//!
//! Размер — сразу в физических пикселях экрана: программный рендерер Slint масштабирует картинки
//! по ближайшему пикселю, и только при 1 : 1 логотип и обложка выглядят чисто.
//!
//! SVG рисуется без внешних ресурсов: ссылки на файлы (в том числе `\\сервер\папка` — сетевые пути
//! Windows) и вложенные картинки не открываются. Размеры картинки проверяются до распаковки.

use anyhow::{Context, Result, bail};
use modly_core::site::{self, ImageFormat};
use resvg::{tiny_skia, usvg};
use slint::{Image, Rgb8Pixel, Rgba8Pixel, SharedPixelBuffer};
use std::io::Cursor;

/// Логотип — круг 56×56 (`ModLogo` в ui/pages.slint).
const LOGO: f32 = 56.0;
/// Обложка — верх карточки высотой 100 (`SiteTile`). Ширина — с запасом на самую широкую карточку
/// сетки (≈460): Slint тогда только обрезает края полосы, а не растягивает её.
const COVER: (f32, f32) = (460.0, 100.0);
/// Обложки игр Steam: плитка библиотеки (ширина — с запасом на широкую сетку) и шапка страницы игры.
const GAME_HEADER: (f32, f32) = (400.0, 165.0);
const GAME_HERO: (f32, f32) = (1400.0, 236.0);
/// Миниатюра карты игры в списке на странице игры (`MapsCard`).
const MAP_THUMB: (f32, f32) = (84.0, 56.0);
/// Фон под прозрачными частями картинок — как у логотипов и обложек на сайте.
const BG: [u8; 3] = [0x0b, 0x0d, 0x14];
/// Больше этого по стороне картинки с сайта не бывают (оригиналы там до 1920 px): 64 МБ в памяти — предел.
const MAX_SIDE: u32 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Logo,
    Cover,
    /// Шапка игры из Steam — верх плитки в библиотеке (`GameTile`, высота 165).
    GameHeader,
    /// Широкий баннер игры из Steam — шапка страницы игры (высота 236).
    GameHero,
    /// Миниатюра карты игры.
    MapThumb,
}

impl Kind {
    /// Размер в пикселях при масштабе экрана `scale` (1.0 = 100 %).
    pub fn size(self, scale: f32) -> (u32, u32) {
        let px = |v: f32| (v * scale.clamp(0.5, 4.0)).round() as u32;
        match self {
            Kind::Logo => (px(LOGO), px(LOGO)),
            Kind::Cover => (px(COVER.0), px(COVER.1)),
            Kind::GameHeader => (px(GAME_HEADER.0), px(GAME_HEADER.1)),
            Kind::GameHero => (px(GAME_HERO.0), px(GAME_HERO.1)),
            Kind::MapThumb => (px(MAP_THUMB.0), px(MAP_THUMB.1)),
        }
    }
}

/// Готовые пиксели: логотип — круг (RGBA, premultiplied), обложка — непрозрачная полоса (RGB).
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub alpha: bool,
}

impl Pixels {
    /// Картинка для Slint (только в потоке интерфейса).
    pub fn to_image(&self) -> Image {
        if self.alpha {
            Image::from_rgba8_premultiplied(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&self.data, self.width, self.height))
        } else {
            Image::from_rgb8(SharedPixelBuffer::<Rgb8Pixel>::clone_from_slice(&self.data, self.width, self.height))
        }
    }
}

/// Декодировать картинку и подогнать её под карточку: `size` — из [`Kind::size`].
/// Картинка заполняет всю площадь, лишнее по краям обрезается — как `object-fit: cover` на сайте.
pub fn decode(data: &[u8], kind: Kind, (w, h): (u32, u32)) -> Result<Pixels> {
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        bail!("неверный размер картинки");
    }
    let rgb = match site::image_format(data) {
        Some(ImageFormat::Svg) => svg(data, w, h)?,
        Some(format) => {
            let (sw, sh, rgb) = raster(data, format)?;
            fit(&rgb, sw, sh, w, h)
        }
        None => bail!("неизвестный формат картинки"),
    };
    Ok(match kind {
        Kind::Logo => Pixels { width: w, height: h, data: circle(&rgb, w, h), alpha: true },
        Kind::Cover | Kind::GameHeader | Kind::GameHero | Kind::MapThumb => Pixels { width: w, height: h, data: rgb, alpha: false },
    })
}

/// Картинка карты целиком, без обрезки: если она больше `max_side` по стороне — уменьшается
/// с сохранением пропорций (метки стоят в долях картинки, поэтому размер не важен).
/// Возвращает и исходный размер картинки (до уменьшения): если мод не задал края карты,
/// координаты его меток — в пикселях исходной картинки.
pub fn decode_full_sized(data: &[u8], max_side: u32) -> Result<(Pixels, (u32, u32))> {
    let format = match site::image_format(data) {
        Some(ImageFormat::Svg) | None => bail!("карта — не PNG, JPEG или WebP"),
        Some(f) => f,
    };
    let (w, h, rgb) = raster(data, format)?;
    let scale = (max_side.max(1) as f32 / w.max(h) as f32).min(1.0);
    let (tw, th) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
    let data = if (tw, th) == (w, h) { rgb } else { fit(&rgb, w, h, tw, th) };
    Ok((Pixels { width: tw, height: th, data, alpha: false }, (w, h)))
}

fn check_size(w: u32, h: u32) -> Result<()> {
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        bail!("картинка {w}×{h} — слишком большая или пустая");
    }
    Ok(())
}

/// PNG, JPEG или WebP → (ширина, высота, непрозрачные RGB).
fn raster(data: &[u8], format: ImageFormat) -> Result<(u32, u32, Vec<u8>)> {
    let (w, h, rgb) = match format {
        ImageFormat::Png => {
            // размер — из заголовка IHDR, до распаковки
            let be = |i: usize| data.get(i..i + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
            check_size(be(16).unwrap_or(0), be(20).unwrap_or(0))?;
            let pixmap = tiny_skia::Pixmap::decode_png(data).context("картинка повреждена")?;
            (pixmap.width(), pixmap.height(), flatten(pixmap.data(), true))
        }
        ImageFormat::Jpeg => {
            use zune_jpeg::zune_core::colorspace::ColorSpace;
            use zune_jpeg::zune_core::options::DecoderOptions;
            let options = DecoderOptions::default()
                .jpeg_set_out_colorspace(ColorSpace::RGBA)
                .set_max_width(MAX_SIDE as usize)
                .set_max_height(MAX_SIDE as usize);
            let mut decoder = zune_jpeg::JpegDecoder::new_with_options(Cursor::new(data), options);
            decoder.decode_headers().ok().context("картинка повреждена")?;
            if decoder.output_colorspace() != Some(ColorSpace::RGBA) {
                bail!("JPEG в этих цветах не поддерживается");
            }
            let rgba = decoder.decode().ok().context("картинка повреждена")?;
            let info = decoder.info().context("картинка повреждена")?;
            (info.width as u32, info.height as u32, flatten(&rgba, false))
        }
        ImageFormat::Webp => {
            let mut decoder = image_webp::WebPDecoder::new(Cursor::new(data)).context("картинка повреждена")?;
            let (w, h) = decoder.dimensions();
            check_size(w, h)?;
            let mut buf = vec![0; decoder.output_buffer_size().context("картинка слишком большая")?];
            decoder.read_image(&mut buf).context("картинка повреждена")?;
            (w, h, if decoder.has_alpha() { flatten(&buf, false) } else { buf })
        }
        ImageFormat::Svg => bail!("SVG — не растровая картинка"),
    };
    check_size(w, h)?;
    if rgb.len() != w as usize * h as usize * 3 {
        bail!("картинка повреждена");
    }
    Ok((w, h, rgb))
}

/// RGBA → RGB на фоне сайта. `premultiplied` — цвета уже умножены на прозрачность.
fn flatten(rgba: &[u8], premultiplied: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 4 * 3);
    for p in rgba.chunks_exact(4) {
        let a = p[3] as u32;
        for i in 0..3 {
            let c = if premultiplied { p[i] as u32 * 255 } else { p[i] as u32 * a };
            out.push(((c + BG[i] as u32 * (255 - a) + 127) / 255).min(255) as u8);
        }
    }
    out
}

/// Какую часть картинки `w`×`h` взять (по центру), чтобы её пропорции были `tw` : `th`.
fn crop_size(w: u32, h: u32, tw: u32, th: u32) -> (u32, u32) {
    let (w64, h64, tw, th) = (w as u64, h as u64, tw as u64, th as u64);
    if w64 * th >= h64 * tw { ((h64 * tw / th).clamp(1, w64) as u32, h) } else { (w, (w64 * th / tw).clamp(1, h64) as u32) }
}

/// RGB `w`×`h` → середина с пропорциями `tw` : `th`, пересчитанная в `tw`×`th`.
fn fit(rgb: &[u8], w: u32, h: u32, tw: u32, th: u32) -> Vec<u8> {
    let (cw, ch) = crop_size(w, h, tw, th);
    let (x0, y0) = (((w - cw) / 2) as usize, ((h - ch) / 2) as usize);
    let (w, tw) = (w as usize, tw as usize);
    // сначала строки по горизонтали, потом столбцы по вертикали
    let across = weights(cw, tw as u32);
    let mut rows = vec![0f32; ch as usize * tw * 3];
    for y in 0..ch as usize {
        let src = &rgb[((y0 + y) * w + x0) * 3..];
        for (x, (first, ws)) in across.iter().enumerate() {
            for c in 0..3 {
                rows[(y * tw + x) * 3 + c] = ws.iter().enumerate().map(|(k, wt)| src[(first + k) * 3 + c] as f32 * wt).sum();
            }
        }
    }
    let down = weights(ch, th);
    let mut out = Vec::with_capacity(tw * th as usize * 3);
    for (first, ws) in &down {
        for i in 0..tw * 3 {
            let v: f32 = ws.iter().enumerate().map(|(k, wt)| rows[(first + k) * tw * 3 + i] * wt).sum();
            out.push(v.round().clamp(0.0, 255.0) as u8);
        }
    }
    out
}

/// Веса пересчёта `src` точек в `dst`: «домик», при уменьшении — во всю ширину исходных точек,
/// чтобы каждая учлась (без ряби на фото). Для каждой новой точки — первая исходная и веса.
fn weights(src: u32, dst: u32) -> Vec<(usize, Vec<f32>)> {
    let ratio = src as f32 / dst as f32;
    let support = ratio.max(1.0);
    (0..dst)
        .map(|i| {
            let center = (i as f32 + 0.5) * ratio;
            let first = (center - support).floor().max(0.0) as usize;
            let last = ((center + support).ceil() as usize).min(src as usize).max(first + 1);
            let mut ws: Vec<f32> =
                (first..last).map(|j| (1.0 - ((j as f32 + 0.5) - center).abs() / support).max(0.0)).collect();
            let sum: f32 = ws.iter().sum();
            if sum > 0.0 {
                ws.iter_mut().for_each(|w| *w /= sum);
            } else {
                ws.iter_mut().for_each(|w| *w = 1.0 / (last - first) as f32);
            }
            (first, ws)
        })
        .collect()
}

/// SVG → непрозрачные RGB размера `w`×`h`.
fn svg(data: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    let text = std::str::from_utf8(data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data))?;
    let opt = usvg::Options {
        // Ни файлов, ни сети, ни вложенных картинок — только фигуры самого логотипа.
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(text, &opt).context("SVG не читается")?;
    let size = tree.size();
    let scale = (w as f32 / size.width()).max(h as f32 / size.height());
    let (dx, dy) = ((w as f32 - size.width() * scale) / 2.0, (h as f32 - size.height() * scale) / 2.0);
    let mut pixmap = tiny_skia::Pixmap::new(w, h).context("SVG")?;
    pixmap.fill(tiny_skia::Color::from_rgba8(BG[0], BG[1], BG[2], 255));
    resvg::render(&tree, tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy), &mut pixmap.as_mut());
    Ok(flatten(pixmap.data(), true))
}

/// RGB → круг RGBA (premultiplied) со сглаженным краем: программный рендерер Slint не обрезает
/// картинки по скруглению, поэтому форма логотипа — в самих пикселях.
fn circle(rgb: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (cx, cy, r) = (w as f32 / 2.0, h as f32 / 2.0, w.min(h) as f32 / 2.0);
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for (i, p) in rgb.chunks_exact(3).enumerate() {
        let (x, y) = ((i as u32 % w) as f32 + 0.5 - cx, (i as u32 / w) as f32 + 0.5 - cy);
        let cover = (r - (x * x + y * y).sqrt() + 0.5).clamp(0.0, 1.0);
        out.extend(p.iter().map(|&c| (c as f32 * cover + 0.5) as u8));
        out.push((255.0 * cover + 0.5) as u8);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::DynamicImage;

    fn encode(img: &image::RgbaImage, format: image::ImageFormat) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        match format {
            image::ImageFormat::Jpeg => DynamicImage::ImageRgba8(img.clone()).to_rgb8().write_to(&mut out, format).unwrap(),
            image::ImageFormat::WebP => image_webp::WebPEncoder::new(&mut out)
                .encode(img.as_raw(), img.width(), img.height(), image_webp::ColorType::Rgba8)
                .unwrap(),
            _ => img.write_to(&mut out, format).unwrap(),
        }
        out.into_inner()
    }

    fn px(p: &Pixels, x: u32, y: u32) -> &[u8] {
        let n = if p.alpha { 4 } else { 3 };
        let i = ((y * p.width + x) * n) as usize;
        &p.data[i..i + n as usize]
    }

    /// Левая половина красная, правая — прозрачная.
    fn sample(w: u32, h: u32) -> image::RgbaImage {
        image::RgbaImage::from_fn(w, h, |x, _| if x < w / 2 { image::Rgba([230, 20, 20, 255]) } else { image::Rgba([0, 0, 0, 0]) })
    }

    #[test]
    fn sizes_follow_the_screen_scale() {
        assert_eq!(Kind::Logo.size(1.0), (56, 56));
        assert_eq!(Kind::Logo.size(1.25), (70, 70));
        assert_eq!(Kind::Cover.size(1.5), (690, 150));
        assert!(decode(&encode(&sample(4, 4), image::ImageFormat::Png), Kind::Cover, (0, 10)).is_err());
    }

    #[test]
    fn logos_become_round() {
        for format in [image::ImageFormat::WebP, image::ImageFormat::Png] {
            let logo = decode(&encode(&sample(300, 200), format), Kind::Logo, (70, 70)).unwrap();
            assert_eq!((logo.width, logo.height, logo.alpha), (70, 70, true));
            assert_eq!(logo.data.len(), 70 * 70 * 4);
            assert_eq!(px(&logo, 0, 0)[3], 0, "угол за кругом прозрачный");
            assert_eq!(px(&logo, 69, 69)[3], 0);
            assert_eq!(px(&logo, 15, 35), &[230, 20, 20, 255], "{format:?}");
            assert_eq!(px(&logo, 55, 35), &[BG[0], BG[1], BG[2], 255], "прозрачное — на фон сайта");
        }
        // маленький логотип плавно увеличивается до нужного размера
        let small = decode(&encode(&sample(20, 20), image::ImageFormat::Png), Kind::Logo, (56, 56)).unwrap();
        assert_eq!((small.width, small.height), (56, 56));
    }

    #[test]
    fn covers_are_cut_to_a_band() {
        let wide = encode(&sample(1600, 900), image::ImageFormat::Jpeg);
        let cover = decode(&wide, Kind::Cover, (460, 100)).unwrap();
        assert_eq!((cover.width, cover.height, cover.alpha), (460, 100, false));
        assert_eq!(cover.data.len(), 460 * 100 * 3);
        let left = px(&cover, 100, 50);
        assert!(left[0] > 200 && left[1] < 50, "{left:?}");
        for (w, h) in [(300, 200), (2000, 50), (1, 1)] {
            let c = decode(&encode(&sample(w, h), image::ImageFormat::WebP), Kind::Cover, (460, 100)).unwrap();
            assert_eq!((c.width, c.height), (460, 100), "{w}×{h}");
        }
    }

    #[test]
    fn shrinking_averages_without_ripple() {
        // шахматка по пикселю при сильном уменьшении — ровный серый, а не узор
        let chess = image::RgbaImage::from_fn(720, 720, |x, y| {
            let v = if (x + y) % 2 == 0 { 255 } else { 0 };
            image::Rgba([v, v, v, 255])
        });
        let cover = decode(&encode(&chess, image::ImageFormat::Png), Kind::Cover, (100, 40)).unwrap();
        assert!(cover.data.iter().all(|&v| (110..=145).contains(&v)), "{:?}", &cover.data[..12]);
        // серый JPEG тоже читается
        let gray = image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(64, 64, image::Luma([200])));
        let mut jpeg = std::io::Cursor::new(Vec::new());
        gray.write_to(&mut jpeg, image::ImageFormat::Jpeg).unwrap();
        let logo = decode(jpeg.get_ref(), Kind::Logo, (32, 32)).unwrap();
        let c = px(&logo, 16, 16);
        assert!(c[0].abs_diff(200) < 4 && c[0] == c[1] && c[1] == c[2], "{c:?}");
    }

    const LOGO_SVG: &str =r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128">
        <defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" stop-color="#9b7bff"/><stop offset="1" stop-color="#ff7ac8"/></linearGradient></defs>
        <circle cx="64" cy="64" r="60" fill="url(#g)"/></svg>"##;

    #[test]
    fn maps_keep_their_shape() {
        let big = encode(&sample(1200, 800), image::ImageFormat::WebP);
        let (m, size) = decode_full_sized(&big, 600).unwrap();
        assert_eq!(size, (1200, 800), "исходный размер — до уменьшения");
        assert_eq!((m.width, m.height, m.alpha), (600, 400, false));
        assert!(px(&m, 100, 200)[0] > 200, "левая половина — красная");
        let small = decode_full_sized(&encode(&sample(300, 200), image::ImageFormat::Png), 2560).unwrap().0;
        assert_eq!((small.width, small.height), (300, 200), "маленькая карта не растягивается");
        assert!(decode_full_sized(LOGO_SVG.as_bytes(), 2560).is_err(), "SVG картой не бывает");
    }

    #[test]
    fn svg_logos_are_drawn() {
        let logo = decode(LOGO_SVG.as_bytes(), Kind::Logo, (56, 56)).unwrap();
        assert_eq!((logo.width, logo.height), (56, 56));
        let c = px(&logo, 28, 28);
        assert!(c[0] > 150 && c[2] > 150 && c[3] == 255, "{c:?}");
        let cover = decode(LOGO_SVG.as_bytes(), Kind::Cover, (460, 100)).unwrap();
        assert_eq!((cover.width, cover.height), (460, 100));
    }

    #[test]
    fn svg_cannot_reach_files() {
        let dir = tempfile::tempdir().unwrap();
        let red = encode(&image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255])), image::ImageFormat::Png);
        let png = dir.path().join("secret.png");
        std::fs::write(&png, &red).unwrap();
        let inline = format!("data:image/png;base64,{}", base64(&red));
        for href in [png.display().to_string(), format!("file:///{}", png.display()), inline] {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 10 10">
                   <image width="10" height="10" xlink:href="{href}"/></svg>"#
            );
            let logo = decode(svg.as_bytes(), Kind::Logo, (56, 56)).unwrap();
            assert_eq!(px(&logo, 28, 28), &[BG[0], BG[1], BG[2], 255], "{href}");
        }
    }

    #[test]
    fn refuses_broken_and_huge_images() {
        let size = (56, 56);
        assert!(decode(b"\x89PNG\r\n\x1a\ngarbage", Kind::Logo, size).is_err());
        assert!(decode(b"RIFF\x10\0\0\0WEBPVP8 junk", Kind::Cover, size).is_err());
        assert!(decode(b"<svg", Kind::Logo, size).is_err());
        assert!(decode(b"hello", Kind::Logo, size).is_err());
        // PNG 100 000 × 100 000 (≈40 ГБ в памяти) — отказ по заголовку, без попытки распаковать
        let mut png = encode(&image::RgbaImage::new(1, 1), image::ImageFormat::Png);
        png[16..24].copy_from_slice(&[0, 1, 0x86, 0xa0, 0, 1, 0x86, 0xa0]);
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        assert!(decode(&png, Kind::Logo, size).is_err());
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }

    fn base64(data: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in data.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                out.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
            }
        }
        out
    }
}
