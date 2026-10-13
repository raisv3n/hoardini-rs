//! Embeds the app icon (generated from assets/icon-256.png) into hoardini-app.exe.

use std::{env, fs::File, path::PathBuf};

use image::{
    codecs::ico::{IcoEncoder, IcoFrame},
    imageops::FilterType,
    ExtendedColorType,
};

fn main() {
    let png = "../../assets/icon-256.png";
    println!("cargo:rerun-if-changed={png}");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let img = image::open(png).expect("icon png");
    let frames: Vec<IcoFrame> = [16u32, 24, 32, 48, 64, 128, 256]
        .iter()
        .map(|&s| {
            let rgba = img.resize_exact(s, s, FilterType::Lanczos3).to_rgba8();
            IcoFrame::as_png(rgba.as_raw(), s, s, ExtendedColorType::Rgba8).expect("ico frame")
        })
        .collect();
    let ico = out.join("hoardini.ico");
    IcoEncoder::new(File::create(&ico).unwrap()).encode_images(&frames).expect("ico");
    let rc = out.join("app.rc");
    std::fs::write(&rc, format!("1 ICON \"{}\"\n", ico.display().to_string().replace('\\', "\\\\"))).unwrap();
    embed_resource::compile(&rc, embed_resource::NONE).manifest_optional().unwrap();
}
