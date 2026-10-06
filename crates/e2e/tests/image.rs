use std::io::Cursor;

use base64::{Engine, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use rmcp::model::ContentBlock;
use telehand_e2e::{Env, json, text};

#[tokio::test]
async fn reading_a_png_returns_image_content() {
    let env = Env::start().await;
    let demo = env.dir("demo");
    let mut png = Vec::new();
    DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 6, Rgb([1, 2, 3])))
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .unwrap();
    std::fs::write(demo.join("pic.png"), &png).unwrap();

    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;
    client.ok("select_project", json!({"name": "demo"})).await;

    let result = client.call("read", json!({"path": "pic.png"})).await;
    assert_ne!(result.is_error, Some(true));
    assert_eq!(text(&result), "Read image file [image/png]");
    let image = result
        .content
        .iter()
        .find_map(|block| match block {
            ContentBlock::Image(image) => Some(image),
            _ => None,
        })
        .expect("image block");
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(STANDARD.decode(&image.data).unwrap(), png);

    runner.stop().await;
}
