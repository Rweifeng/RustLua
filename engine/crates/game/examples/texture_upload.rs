use engine_core::texture::{TextureAsset, TextureColorSpace};

const PNG: &[u8] = include_bytes!("../../../../game_test/assets/textures/red-1x1.png");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    pollster::block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let asset = TextureAsset::from_png_bytes(PNG, TextureColorSpace::Srgb)?;
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .map_err(|error| format!("no adapter available: {error}"))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("m3-02-texture-upload-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await?;
    let format = match asset.color_space() {
        TextureColorSpace::Srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        TextureColorSpace::Linear => wgpu::TextureFormat::Rgba8Unorm,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("m3-02-uploaded-png"),
        size: wgpu::Extent3d {
            width: asset.width(),
            height: asset.height(),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        asset.pixels(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(asset.bytes_per_row()),
            rows_per_image: Some(asset.height()),
        },
        wgpu::Extent3d {
            width: asset.width(),
            height: asset.height(),
            depth_or_array_layers: 1,
        },
    );
    let _view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    })?;
    println!(
        "texture_upload_ok adapter={:?} dimensions={}x{} format={format:?} bytes={} color_space={:?}",
        adapter.get_info().name,
        asset.width(),
        asset.height(),
        asset.pixels().len(),
        asset.color_space()
    );
    Ok(())
}
