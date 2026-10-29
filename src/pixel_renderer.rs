use crate::{
    math::Vec3,
    scene::{Scene, ShadingMode},
};

#[derive(Debug, Clone)]
pub struct PixelFrame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub struct PixelStyle {
    pub background: [u8; 3],
    pub accent: [u8; 3],
    pub muted: [u8; 3],
}

#[derive(Clone, Copy)]
struct Point {
    x: f32,
    y: f32,
    z: f32,
}

struct Projector {
    position: Vec3,
    right: Vec3,
    up: Vec3,
    forward: Vec3,
    focus: f32,
    width: usize,
    height: usize,
    perspective: bool,
}

impl Projector {
    fn new(scene: &Scene, width: usize, height: usize) -> Self {
        let yaw = scene.camera.yaw.to_radians();
        let pitch = scene.camera.pitch.to_radians();
        let offset = Vec3::new(
            yaw.sin() * pitch.cos(),
            yaw.cos() * pitch.cos(),
            pitch.sin(),
        ) * scene.camera.distance;
        let position = scene.camera.target + offset;
        let forward = (scene.camera.target - position).normalized();
        let right = Vec3::new(0.0, 0.0, 1.0).cross(forward).normalized();
        let up = forward.cross(right).normalized();
        Self {
            position,
            right,
            up,
            forward,
            focus: scene.camera.distance,
            width,
            height,
            perspective: scene.camera.perspective,
        }
    }

    fn project(&self, world: Vec3) -> Option<Point> {
        let relative = world - self.position;
        let x = relative.dot(self.right);
        let y = relative.dot(self.up);
        let z = relative.dot(self.forward);
        if z <= 0.05 {
            return None;
        }
        let scale = self.height as f32 * 0.95;
        let divisor = if self.perspective {
            self.focus * 0.55 + z * 0.45
        } else {
            self.focus.max(1.0)
        };
        Some(Point {
            x: self.width as f32 * 0.5 + x / divisor * scale,
            y: self.height as f32 * 0.5 - y / divisor * scale,
            z,
        })
    }
}

pub fn render(scene: &Scene, width: u32, height: u32, style: PixelStyle) -> PixelFrame {
    let low_power =
        std::env::var("FORGETUI_QUALITY").is_ok_and(|value| value.eq_ignore_ascii_case("low"));
    let width = width.clamp(16, if low_power { 640 } else { 1200 }) as usize;
    let height = height.clamp(16, if low_power { 480 } else { 800 }) as usize;
    let scale = if !low_power && width * height <= 400_000 {
        2
    } else {
        1
    };
    let high_width = width * scale;
    let high_height = height * scale;
    let projector = Projector::new(scene, high_width, high_height);
    let mut pixels = vec![style.background; high_width * high_height];
    let mut depth = vec![f32::INFINITY; high_width * high_height];

    draw_grid(&projector, &mut pixels, &mut depth, style.muted);
    let light = Vec3::new(-0.35, 0.8, -0.5).normalized();
    for (index, object, triangle) in scene.world_triangles() {
        let Some(points) = project_triangle(&projector, triangle) else {
            continue;
        };
        let normal = (triangle[1] - triangle[0])
            .cross(triangle[2] - triangle[0])
            .normalized();
        let facing = normal.dot(projector.forward * -1.0);
        let selected = index == scene.selected;
        let base = object_color(object.id, style.accent);
        match scene.shading {
            ShadingMode::Solid if facing > 0.0 => {
                let brightness = (normal.dot(light).abs() * 0.7 + 0.25).clamp(0.0, 1.0);
                fill_triangle(
                    high_width,
                    high_height,
                    &mut pixels,
                    &mut depth,
                    points,
                    shade(base, brightness),
                );
                if selected {
                    draw_triangle(
                        high_width,
                        high_height,
                        &mut pixels,
                        &mut depth,
                        points,
                        style.accent,
                        true,
                    );
                }
            }
            ShadingMode::Wireframe => draw_triangle(
                high_width,
                high_height,
                &mut pixels,
                &mut depth,
                points,
                if selected { style.accent } else { base },
                true,
            ),
            ShadingMode::XRay => draw_triangle(
                high_width,
                high_height,
                &mut pixels,
                &mut depth,
                points,
                if selected { style.accent } else { base },
                false,
            ),
            ShadingMode::Solid => {}
        }
    }

    let rgb = if scale == 2 {
        downsample(&pixels, high_width, width, height)
    } else {
        pixels.into_iter().flatten().collect()
    };
    PixelFrame {
        width: width as u32,
        height: height as u32,
        rgb,
    }
}

fn project_triangle(projector: &Projector, triangle: [Vec3; 3]) -> Option<[Point; 3]> {
    Some([
        projector.project(triangle[0])?,
        projector.project(triangle[1])?,
        projector.project(triangle[2])?,
    ])
}

fn edge(a: Point, b: Point, x: f32, y: f32) -> f32 {
    (x - a.x) * (b.y - a.y) - (y - a.y) * (b.x - a.x)
}

fn fill_triangle(
    width: usize,
    height: usize,
    pixels: &mut [[u8; 3]],
    depth: &mut [f32],
    points: [Point; 3],
    color: [u8; 3],
) {
    let min_x = points
        .iter()
        .map(|p| p.x)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as usize;
    let max_x = points
        .iter()
        .map(|p| p.x)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(width as f32 - 1.0) as usize;
    let min_y = points
        .iter()
        .map(|p| p.y)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as usize;
    let max_y = points
        .iter()
        .map(|p| p.y)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(height as f32 - 1.0) as usize;
    let area = edge(points[0], points[1], points[2].x, points[2].y);
    if area.abs() < 0.001 {
        return;
    }
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let a = edge(points[1], points[2], px, py) / area;
            let b = edge(points[2], points[0], px, py) / area;
            let c = edge(points[0], points[1], px, py) / area;
            if a >= 0.0 && b >= 0.0 && c >= 0.0 {
                let z = points[0].z * a + points[1].z * b + points[2].z * c;
                let offset = y * width + x;
                if z < depth[offset] {
                    depth[offset] = z;
                    pixels[offset] = color;
                }
            }
        }
    }
}

fn draw_triangle(
    width: usize,
    height: usize,
    pixels: &mut [[u8; 3]],
    depth: &mut [f32],
    p: [Point; 3],
    color: [u8; 3],
    depth_test: bool,
) {
    draw_line(
        (width, height),
        pixels,
        depth,
        p[0],
        p[1],
        color,
        depth_test,
    );
    draw_line(
        (width, height),
        pixels,
        depth,
        p[1],
        p[2],
        color,
        depth_test,
    );
    draw_line(
        (width, height),
        pixels,
        depth,
        p[2],
        p[0],
        color,
        depth_test,
    );
}

fn draw_line(
    size: (usize, usize),
    pixels: &mut [[u8; 3]],
    depth: &mut [f32],
    a: Point,
    b: Point,
    color: [u8; 3],
    depth_test: bool,
) {
    let (width, height) = size;
    let steps = (a.x - b.x).abs().max((a.y - b.y).abs()).ceil().max(1.0) as usize;
    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        let x = (a.x + (b.x - a.x) * t).round() as i32;
        let y = (a.y + (b.y - a.y) * t).round() as i32;
        if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
            continue;
        }
        let offset = y as usize * width + x as usize;
        let z = a.z + (b.z - a.z) * t - 0.002;
        if !depth_test || z < depth[offset] {
            depth[offset] = z;
            pixels[offset] = color;
        }
    }
}

fn draw_grid(projector: &Projector, pixels: &mut [[u8; 3]], depth: &mut [f32], color: [u8; 3]) {
    for coordinate in -11..=11 {
        let c = coordinate as f32 * 10.0;
        for (a, b) in [
            (Vec3::new(c, -110.0, 0.0), Vec3::new(c, 110.0, 0.0)),
            (Vec3::new(-110.0, c, 0.0), Vec3::new(110.0, c, 0.0)),
        ] {
            if let (Some(a), Some(b)) = (projector.project(a), projector.project(b)) {
                draw_line(
                    (projector.width, projector.height),
                    pixels,
                    depth,
                    a,
                    b,
                    color,
                    true,
                );
            }
        }
    }
}

fn object_color(id: u64, accent: [u8; 3]) -> [u8; 3] {
    let choices = [
        [74, 151, 213],
        [218, 112, 83],
        [104, 188, 120],
        [173, 120, 214],
        [205, 178, 84],
    ];
    let mut color = choices[id as usize % choices.len()];
    for i in 0..3 {
        color[i] = ((u16::from(color[i]) * 3 + u16::from(accent[i])) / 4) as u8;
    }
    color
}

fn shade(color: [u8; 3], brightness: f32) -> [u8; 3] {
    color.map(|channel| (f32::from(channel) * brightness).min(255.0) as u8)
}

fn downsample(pixels: &[[u8; 3]], high_width: usize, width: usize, height: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        for x in 0..width {
            for channel in 0..3 {
                let sum = u16::from(pixels[(y * 2) * high_width + x * 2][channel])
                    + u16::from(pixels[(y * 2) * high_width + x * 2 + 1][channel])
                    + u16::from(pixels[(y * 2 + 1) * high_width + x * 2][channel])
                    + u16::from(pixels[(y * 2 + 1) * high_width + x * 2 + 1][channel]);
                output.push((sum / 4) as u8);
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_renderer_returns_rgb_image() {
        let frame = render(
            &Scene::default(),
            160,
            100,
            PixelStyle {
                background: [0, 0, 0],
                accent: [0, 255, 255],
                muted: [30, 30, 30],
            },
        );
        assert_eq!(frame.rgb.len(), 160 * 100 * 3);
        assert!(frame.rgb.chunks_exact(3).any(|pixel| pixel != [0, 0, 0]));
    }
}
