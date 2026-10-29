use crate::{
    math::Vec3,
    scene::{Scene, ShadingMode},
};

const RAMP: &[u8] = b" .:-=+*#%@";

#[derive(Debug, Clone)]
pub struct RenderedFrame {
    pub rows: Vec<String>,
    pub triangles: usize,
    pub width: usize,
    pub height: usize,
    object_map: Vec<Option<usize>>,
}

impl RenderedFrame {
    pub fn object_at(&self, x: usize, y: usize) -> Option<usize> {
        if x >= self.width || y >= self.height {
            None
        } else {
            self.object_map[y * self.width + x]
        }
    }
}

#[derive(Clone, Copy)]
struct Cell {
    glyph: char,
    depth: f32,
    object: Option<usize>,
}

#[derive(Clone, Copy)]
struct Projected {
    x: f32,
    y: f32,
    depth: f32,
}

struct FrameBuffer {
    width: usize,
    height: usize,
    cells: Vec<Cell>,
}

impl FrameBuffer {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![
                Cell {
                    glyph: ' ',
                    depth: f32::INFINITY,
                    object: None,
                };
                width * height
            ],
        }
    }

    fn put(
        &mut self,
        x: i32,
        y: i32,
        depth: f32,
        glyph: char,
        depth_test: bool,
        object: Option<usize>,
    ) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let index = y as usize * self.width + x as usize;
        if !depth_test || depth < self.cells[index].depth {
            self.cells[index] = Cell {
                glyph,
                depth,
                object,
            };
        }
    }

    fn finish(self) -> (Vec<String>, Vec<Option<usize>>) {
        let rows = self
            .cells
            .chunks(self.width)
            .map(|row| row.iter().map(|cell| cell.glyph).collect())
            .collect();
        let object_map = self.cells.iter().map(|cell| cell.object).collect();
        (rows, object_map)
    }
}

struct Projector {
    position: Vec3,
    right: Vec3,
    up: Vec3,
    forward: Vec3,
    width: usize,
    height: usize,
    perspective: bool,
    focus_distance: f32,
}

impl Projector {
    fn for_scene(scene: &Scene, width: usize, height: usize) -> Self {
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
            width,
            height,
            perspective: scene.camera.perspective,
            focus_distance: scene.camera.distance,
        }
    }

    fn project(&self, point: Vec3) -> Option<Projected> {
        let relative = point - self.position;
        let x = relative.dot(self.right);
        let y = relative.dot(self.up);
        let depth = relative.dot(self.forward);
        if depth <= 0.05 {
            return None;
        }
        let (screen_x, screen_y) = if self.perspective {
            let focal = self.height as f32 * 0.9;
            let gentle_depth = self.focus_distance * 0.55 + depth * 0.45;
            (
                self.width as f32 * 0.5 + x / gentle_depth * focal,
                self.height as f32 * 0.5 - y / gentle_depth * focal * 0.5,
            )
        } else {
            let scale = self.height as f32 / self.focus_distance.max(1.0);
            (
                self.width as f32 * 0.5 + x * scale,
                self.height as f32 * 0.5 - y * scale * 0.5,
            )
        };
        Some(Projected {
            x: screen_x,
            y: screen_y,
            depth,
        })
    }
}

pub fn render_scene(scene: &Scene, width: u16, height: u16) -> RenderedFrame {
    let width = usize::from(width.max(1));
    let height = usize::from(height.max(1));
    let mut buffer = FrameBuffer::new(width, height);
    let projector = Projector::for_scene(scene, width, height);

    render_grid(&mut buffer, &projector);
    let light = Vec3::new(-0.4, 0.8, -0.5).normalized();
    let mut count = 0;

    for (object_index, object, triangle) in scene.world_triangles() {
        let Some(points) = project_triangle(&projector, triangle) else {
            continue;
        };
        count += 1;
        let normal = (triangle[1] - triangle[0])
            .cross(triangle[2] - triangle[0])
            .normalized();
        let facing = normal.dot(projector.forward * -1.0);
        let selected = scene
            .selected()
            .is_some_and(|selected| selected.id == object.id);

        match scene.shading {
            ShadingMode::Solid => {
                if facing > 0.0 {
                    let intensity = (normal.dot(light).abs() * 0.72 + 0.18).clamp(0.0, 1.0);
                    let glyph = RAMP[(intensity * (RAMP.len() - 1) as f32) as usize] as char;
                    fill_triangle(&mut buffer, points, glyph, object_index);
                    if selected {
                        draw_triangle(&mut buffer, points, '·', true, Some(object_index));
                    }
                }
            }
            ShadingMode::Wireframe => draw_triangle(
                &mut buffer,
                points,
                if selected { '@' } else { '#' },
                true,
                Some(object_index),
            ),
            ShadingMode::XRay => draw_triangle(
                &mut buffer,
                points,
                if selected { '@' } else { '+' },
                false,
                Some(object_index),
            ),
        }
    }

    let (rows, object_map) = buffer.finish();
    RenderedFrame {
        rows,
        triangles: count,
        width,
        height,
        object_map,
    }
}

fn project_triangle(projector: &Projector, triangle: [Vec3; 3]) -> Option<[Projected; 3]> {
    Some([
        projector.project(triangle[0])?,
        projector.project(triangle[1])?,
        projector.project(triangle[2])?,
    ])
}

fn edge(a: Projected, b: Projected, x: f32, y: f32) -> f32 {
    (x - a.x) * (b.y - a.y) - (y - a.y) * (b.x - a.x)
}

fn fill_triangle(
    buffer: &mut FrameBuffer,
    points: [Projected; 3],
    glyph: char,
    object_index: usize,
) {
    let min_x = points
        .iter()
        .map(|p| p.x)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as i32;
    let max_x = points
        .iter()
        .map(|p| p.x)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(buffer.width as f32 - 1.0) as i32;
    let min_y = points
        .iter()
        .map(|p| p.y)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as i32;
    let max_y = points
        .iter()
        .map(|p| p.y)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(buffer.height as f32 - 1.0) as i32;
    let area = edge(points[0], points[1], points[2].x, points[2].y);
    if area.abs() < 0.001 {
        return;
    }

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let w0 = edge(points[1], points[2], px, py) / area;
            let w1 = edge(points[2], points[0], px, py) / area;
            let w2 = edge(points[0], points[1], px, py) / area;
            if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                let depth = points[0].depth * w0 + points[1].depth * w1 + points[2].depth * w2;
                buffer.put(x, y, depth, glyph, true, Some(object_index));
            }
        }
    }
}

fn draw_triangle(
    buffer: &mut FrameBuffer,
    points: [Projected; 3],
    glyph: char,
    depth_test: bool,
    object: Option<usize>,
) {
    draw_line(buffer, points[0], points[1], glyph, depth_test, object);
    draw_line(buffer, points[1], points[2], glyph, depth_test, object);
    draw_line(buffer, points[2], points[0], glyph, depth_test, object);
}

fn draw_line(
    buffer: &mut FrameBuffer,
    a: Projected,
    b: Projected,
    glyph: char,
    depth_test: bool,
    object: Option<usize>,
) {
    let steps = (a.x - b.x).abs().max((a.y - b.y).abs()).ceil().max(1.0) as usize;
    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        buffer.put(
            (a.x + (b.x - a.x) * t).round() as i32,
            (a.y + (b.y - a.y) * t).round() as i32,
            a.depth + (b.depth - a.depth) * t - 0.001,
            glyph,
            depth_test,
            object,
        );
    }
}

fn render_grid(buffer: &mut FrameBuffer, projector: &Projector) {
    for coordinate in -11..=11 {
        let c = coordinate as f32 * 10.0;
        if let (Some(a), Some(b)) = (
            projector.project(Vec3::new(c, -110.0, 0.0)),
            projector.project(Vec3::new(c, 110.0, 0.0)),
        ) {
            draw_line(
                buffer,
                a,
                b,
                if coordinate == 0 { 'y' } else { '.' },
                true,
                None,
            );
        }
        if let (Some(a), Some(b)) = (
            projector.project(Vec3::new(-110.0, c, 0.0)),
            projector.project(Vec3::new(110.0, c, 0.0)),
        ) {
            draw_line(
                buffer,
                a,
                b,
                if coordinate == 0 { 'x' } else { '.' },
                true,
                None,
            );
        }
    }
    if let (Some(a), Some(b)) = (
        projector.project(Vec3::ZERO),
        projector.project(Vec3::new(0.0, 0.0, 20.0)),
    ) {
        draw_line(buffer, a, b, 'z', true, None);
    }
}
