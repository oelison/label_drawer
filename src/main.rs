#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::{engine::general_purpose, Engine as _};
use serde_json::json;
use reqwest::blocking::Client;
use image::{GenericImageView, Luma, ImageBuffer, imageops::{self, FilterType, dither, BiLevel}};
use rusttype::{Font, Scale};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer, ModelRc, VecModel, SharedString};
use std::{fs, fs::File, io::Read};
use ttf_parser::Face;
use rfd::FileDialog;
use std::env;

use std::error::Error;
use std::path::PathBuf;
use std::path::Path;
use std::collections::HashSet;

slint::include_modules!();

#[derive(Debug, Clone)]
struct FontEntry {
    display_name: SharedString,
    path: String,
}

/// helper function: find all subdirectories of a directory
fn find_subdirs_recursively(dir: &Path) -> Vec<PathBuf> {
    let mut subdirs = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                subdirs.push(path.clone());
                // add recursiv subdirs
                subdirs.extend(find_subdirs_recursively(&path));
            }
        }
    }
    subdirs
}

/// get list of common font directories for the current OS
fn get_system_font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    match std::env::consts::OS {
        // ---------------- Windows ----------------
        "windows" => {
            dirs.push(PathBuf::from("C:\\Windows\\Fonts"));
        }

        // ---------------- macOS ----------------
        "macos" => {
            dirs.push(PathBuf::from("/System/Library/Fonts"));
            dirs.push(PathBuf::from("/System/Library/Fonts/Supplemental"));
            if let Some(home) = dirs_next::home_dir() {
                dirs.push(home.join("Library/Fonts"));
            }
        }

        // ---------------- Linux & BSD ----------------
        "linux" | "freebsd" | "openbsd" | "netbsd" => {
            dirs.push(PathBuf::from("/usr/share/fonts"));
            dirs.push(PathBuf::from("/usr/local/share/fonts"));
            if let Some(home) = dirs_next::home_dir() {
                dirs.push(home.join(".fonts"));
                dirs.push(home.join(".local/share/fonts"));
            }
        }

        // ---------------- Fallback ----------------
        other => {
            eprintln!("unknown OS: {} – no font dirs estimated", other);
        }
    }

    let mut all_dirs = Vec::new();
    for dir in &dirs {
        all_dirs.push(dir.clone());
        all_dirs.extend(find_subdirs_recursively(dir));
    }

    all_dirs
}

fn main() -> Result<(), Box<dyn Error>> {
    let ui = AppWindow::new()?;

    // scan fonts
    let font_dirs = get_system_font_dirs();
    let mut font_entries: Vec<FontEntry> = Vec::new();
    let mut font_names: Vec<SharedString> = Vec::new();
    let mut seen_fonts = HashSet::new();
    for font_dir in font_dirs {
        println!("Scan folder: {}", font_dir.display());
        if let Ok(entries) = fs::read_dir(font_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ext.eq_ignore_ascii_case("ttf") {
                        // load font file
                        if let Ok(mut file) = File::open(&path) {
                            let mut data = Vec::new();
                            if file.read_to_end(&mut data).is_ok() {
                                if let Ok(face) = Face::parse(&data, 0) {
                                    // font name extraction
                                    let name = face
                                        .names()
                                        .into_iter()
                                        .find(|n| n.name_id == ttf_parser::name_id::FULL_NAME)
                                        .and_then(|n| n.to_string())
                                        .unwrap_or_else(|| {
                                            path.file_stem()
                                                .unwrap_or_default()
                                                .to_string_lossy()
                                                .into_owned()
                                        });
                                    // deduplicate by name
                                    if seen_fonts.insert(name.clone()) {
                                        let entry = FontEntry {
                                            display_name: SharedString::from(name.clone()),
                                            path: path.to_string_lossy().into_owned(),
                                        };

                                        font_entries.push(entry);
                                        font_names.push(SharedString::from(name));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    // sort alphabetically by display_name
    font_entries.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    let font_names: Vec<SharedString> = font_entries
        .iter()
        .map(|entry| entry.display_name.clone())
        .collect();


    // set font names in UI
    ui.set_fonts(ModelRc::new(VecModel::from(font_names)));

    ui.on_request_create_label({
        let ui_handle = ui.as_weak();
        let font_entries = font_entries.clone();
        move || {
            let ui = ui_handle.unwrap();
            let label_text = ui.get_label_text();
            let font_index = ui.get_font_index();
            let mut font_path = String::from("/usr/share/fonts/truetype/msttcorefonts/arial.ttf"); // Default

            if let Some(entry) = font_entries.get(font_index as usize) {
                font_path = entry.path.clone();
                println!(
                    "Label '{}' with font: {} (Path: {})",
                    label_text, entry.display_name, entry.path
                );
            }

            println!("Label '{}' with font: {}", label_text, font_path);

            let width = 2000;
            let height = 96;
            let mut used_len = 0;
            let img: ImageBuffer<Luma<u8>, Vec<u8>> = create_image_with_text(width, height,label_text.as_str(), font_path.as_str(), &mut used_len);
            let byte_data =  get_bitmap_data(img.clone(), height, width);
            let _ = write_image(byte_data);
            ui.set_print_width(used_len as i32);
            let slint_image = get_slint_img(img, height as u32, width as u32);
            ui.set_previewimage(slint_image);
        }
    });
    ui.on_request_print_label({
        let ui_handle = ui.as_weak();
        move || {
            let ui = ui_handle.unwrap();
            let length = ui.get_print_width();
            let _ = print_image(length as u32);
        }
    });
    ui.on_load_image({
        let ui_handle = ui.as_weak();
        move || {
            let ui = ui_handle.unwrap();
            let mut image_path = ui.get_image_path().to_string();
            let mut start_folder = "";
            if env::consts::OS == "windows" {
                start_folder = "C:\\Users";
            } else if env::consts::OS == "linux" {
                start_folder = "~/";
            } else if env::consts::OS == "macos" {
                start_folder = "/Users";
            }
            if image_path.is_empty() || !Path::new(&image_path).exists() {
                let file_dialog = FileDialog::new()
                    .add_filter("Image Files", &["png", "jpg", "jpeg", "bmp", "gif"])
                    .set_directory(start_folder)
                    .set_title("pick image file")
                    .pick_file();

                let file_name = file_dialog
                    .as_ref()
                    .and_then(|path| path.to_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| "ups".to_string());
                ui.set_image_path(file_name.clone().into());
                println!("image path: {}", file_name);
                image_path = file_name;
            }
            println!("Load image: {}", image_path);
            if Path::new(&image_path).exists() {
                let img = image::open(&image_path);
                match img {
                    Ok(img) => {
                        // target size
                        let target_height = 96u32;
                        let target_width = 2000u32;

                        // scaler proportionally
                        let (orig_w, orig_h) = img.dimensions();
                        let scale = target_height as f32 / orig_h as f32;
                        let new_w = (orig_w as f32 * scale).round() as u32;

                        // scale image
                        let resized = img.resize_exact(new_w, target_height, FilterType::Lanczos3).to_luma8();

                        // dithern
                        let mut dithered = resized.clone();
                        dither(&mut dithered, &BiLevel);

                        // create final image with white background
                        let mut final_img = ImageBuffer::from_pixel(target_width, target_height, Luma([255u8]));
                        imageops::replace(&mut final_img, &dithered, 0, 0);

                        let byte_data = get_bitmap_data(final_img.clone(), target_height as usize, target_width as usize);
                        let _ = write_image(byte_data);
                        ui.set_print_width(new_w as i32);
                        let slint_image = get_slint_img(final_img, target_height, target_width);
                        ui.set_previewimage(slint_image);
                        ui.set_image_path(image_path.into());
                    }
                    Err(e) => {
                        eprintln!("Error during loading the image: {}", e);
                    }
                }
            } else {
                eprintln!("Path do not exist: {}", image_path);
                ui.set_image_path("".into());
            }
        }
    });
    ui.run()?;
    Ok(())
}

fn create_image_with_text(width: usize, height: usize, text: &str, font_path: &str, used_len: &mut usize) -> ImageBuffer<Luma<u8>, Vec<u8>> {
    // image buffer
    *used_len = 0;
    // create white image
    let mut img = ImageBuffer::from_pixel(width as u32, height as u32, Luma([255u8]));
    // load font
    let font_data = fs::read(font_path).expect("Error reading font file");
    let font = Font::try_from_bytes(&font_data).unwrap();

    // scale the font
    let scale = Scale { x: 96.0, y: 96.0 };

    // Text start position
    let start = rusttype::point(10.0, 71.0);

    // draw the text
    for glyph in font.layout(text, scale, start) {
        if let Some(bb) = glyph.pixel_bounding_box() {
            glyph.draw(|x, y, v| {
                let px = bb.min.x + x as i32;
                let py = bb.min.y + y as i32;
                if px >= 0 && px < width as i32 && py >= 0 && py < height as i32 {
                    let pixel: &mut Luma<u8> = img.get_pixel_mut(px as u32, py as u32);
                    if v > 0.5 {
                        pixel[0] = 0; // black
                        if px as usize > *used_len {
                            *used_len = px as usize;
                        }
                    }
                }
            });
        }
    }
    *used_len += 1;
    img
}

fn get_bitmap_data(img: ImageBuffer<Luma<u8>, Vec<u8>>, height: usize, width: usize) -> Vec<u8> { 
    let mut packed: Vec<u8> = Vec::with_capacity((width * height).div_ceil(8));
    let mut current_byte = 0u8;
    let mut bit_pos = 0;
    println!("Image dimensions: WxH {}x{}", width, height);
    for x in 0..width {
        for y in 0..height {
            let Luma([val]) = *img.get_pixel(x as u32, height as u32 - y as u32 - 1);
            let bit = if val < 128 { 1 } else { 0 };
            current_byte |= bit << (7 - bit_pos);
            bit_pos += 1;

            if bit_pos == 8 {
                packed.push(current_byte);
                current_byte = 0;
                bit_pos = 0;
            }
        }
    }
    println!("Bit-packed length: {} bytes", packed.len());
    packed
}

fn get_slint_img (img: ImageBuffer<Luma<u8>, Vec<u8>>, height: u32, width: u32) -> slint::Image {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    {
        let pixels = buffer.make_mut_bytes();
        for (i, Luma([val])) in img.pixels().enumerate() {
            let offset = i * 4;
            pixels[offset] = *val;     // R
            pixels[offset + 1] = *val; // G
            pixels[offset + 2] = *val; // B
            pixels[offset + 3] = 255;  // A
        }
    }
    Image::from_rgba8(buffer)
}

fn write_image(bytesvec: Vec<u8>) -> Result<(), Box<dyn Error>> {
    const CHUNK_SIZE: usize = 96;

    let client = Client::new();

    let mut index = 0;

    for chunk in bytesvec.chunks(CHUNK_SIZE) {
        let b64 = general_purpose::STANDARD.encode(chunk);
        // 3 JSON erzeugen
        let body = json!({
            "index": index,
            "data": b64
        });
        // 4 HTTP POST Request an /uploadjson
    
        let response = client.post("http://192.168.54.148/uploadjson")
            .json(&body)
            .send()?;

        if !response.status().is_success() {
            eprintln!("Error at UploadJson, Index {}: {}", index, response.status());
            return Err("Upload failed".into());
        }
        index += CHUNK_SIZE;
    }
    
    println!("UploadJson successfully in {} blocks!", index);

    Ok(())

}

fn print_image(length: u32) -> Result<(), Box<dyn Error>> {
    let client = Client::new();
    client.get(format!("http://192.168.54.148/print?length={}", length))
        .send()?;

    Ok(())
}