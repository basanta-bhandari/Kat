use crate::{
    math::Vec3,
    scene::{Mesh, Object, Scene},
};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct PrintReport {
    pub triangles: usize,
    pub degenerate_triangles: usize,
    pub boundary_edges: usize,
    pub non_manifold_edges: usize,
    pub overhang_faces: usize,
    pub volume_mm3: f32,
    pub bounds: Option<(Vec3, Vec3)>,
    pub fits_build_volume: bool,
    pub below_bed: bool,
}

impl PrintReport {
    pub fn is_printable(&self) -> bool {
        self.triangles > 0
            && self.degenerate_triangles == 0
            && self.boundary_edges == 0
            && self.non_manifold_edges == 0
            && self.fits_build_volume
            && !self.below_bed
    }

    pub fn summary(&self) -> String {
        let verdict = if self.is_printable() {
            "PRINTABLE"
        } else {
            "NEEDS WORK"
        };
        format!(
            "{verdict} · {} tris · {:.1} cm³ · {} open edges · {} non-manifold · {} degenerate · {} overhangs{}{}",
            self.triangles,
            self.volume_mm3 / 1000.0,
            self.boundary_edges,
            self.non_manifold_edges,
            self.degenerate_triangles,
            self.overhang_faces,
            if self.below_bed { " · below bed" } else { "" },
            if self.fits_build_volume {
                ""
            } else {
                " · outside build volume"
            },
        )
    }
}

pub fn validate(scene: &Scene) -> PrintReport {
    let mut report = PrintReport {
        triangles: 0,
        degenerate_triangles: 0,
        boundary_edges: 0,
        non_manifold_edges: 0,
        overhang_faces: 0,
        volume_mm3: 0.0,
        bounds: None,
        fits_build_volume: true,
        below_bed: false,
    };
    let mut global_min = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut global_max = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);

    for object in scene.objects.iter().filter(|object| object.visible) {
        let mut edges: HashMap<(usize, usize), usize> = HashMap::new();
        let mut signed_volume = 0.0;
        for face in &object.mesh.triangles {
            report.triangles += 1;
            if face
                .iter()
                .any(|&index| index >= object.mesh.vertices.len())
            {
                report.degenerate_triangles += 1;
                continue;
            }
            for (a, b) in [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])] {
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            }
            let a = object.transform.point(object.mesh.vertices[face[0]]);
            let b = object.transform.point(object.mesh.vertices[face[1]]);
            let c = object.transform.point(object.mesh.vertices[face[2]]);
            let cross = (b - a).cross(c - a);
            if cross.dot(cross) < 0.000001 {
                report.degenerate_triangles += 1;
            } else if cross.normalized().z < -0.707 {
                report.overhang_faces += 1;
            }
            signed_volume += a.dot(b.cross(c)) / 6.0;
            for point in [a, b, c] {
                global_min.x = global_min.x.min(point.x);
                global_min.y = global_min.y.min(point.y);
                global_min.z = global_min.z.min(point.z);
                global_max.x = global_max.x.max(point.x);
                global_max.y = global_max.y.max(point.y);
                global_max.z = global_max.z.max(point.z);
            }
        }
        report.volume_mm3 += signed_volume.abs();
        report.boundary_edges += edges.values().filter(|&&count| count == 1).count();
        report.non_manifold_edges += edges.values().filter(|&&count| count > 2).count();
    }

    if report.triangles > 0 {
        report.bounds = Some((global_min, global_max));
        let half_x = scene.build_volume.x * 0.5;
        let half_y = scene.build_volume.y * 0.5;
        report.below_bed = global_min.z < -0.001;
        report.fits_build_volume = global_min.x >= -half_x
            && global_max.x <= half_x
            && global_min.y >= -half_y
            && global_max.y <= half_y
            && global_max.z <= scene.build_volume.z
            && !report.below_bed;
    }
    report
}

pub fn place_on_bed(object: &mut Object) -> bool {
    let Some((min, _)) = object.world_bounds() else {
        return false;
    };
    object.transform.position.z -= min.z;
    true
}

pub fn center_on_bed(object: &mut Object) -> bool {
    let Some((min, max)) = object.world_bounds() else {
        return false;
    };
    object.transform.position.x -= (min.x + max.x) * 0.5;
    object.transform.position.y -= (min.y + max.y) * 0.5;
    place_on_bed(object)
}

pub fn repair_mesh(mesh: &mut Mesh) -> (usize, usize) {
    const TOLERANCE: f32 = 0.001;
    let old_vertices = mesh.vertices.len();
    let old_triangles = mesh.triangles.len();
    let mut welded = Vec::new();
    let mut lookup = HashMap::new();
    let mut remap = Vec::with_capacity(old_vertices);
    for vertex in &mesh.vertices {
        let key = (
            (vertex.x / TOLERANCE).round() as i64,
            (vertex.y / TOLERANCE).round() as i64,
            (vertex.z / TOLERANCE).round() as i64,
        );
        let index = *lookup.entry(key).or_insert_with(|| {
            welded.push(*vertex);
            welded.len() - 1
        });
        remap.push(index);
    }

    let mut seen = HashSet::new();
    let mut triangles = Vec::new();
    for face in &mesh.triangles {
        if face.iter().any(|&index| index >= remap.len()) {
            continue;
        }
        let mapped = [remap[face[0]], remap[face[1]], remap[face[2]]];
        if mapped[0] == mapped[1] || mapped[1] == mapped[2] || mapped[2] == mapped[0] {
            continue;
        }
        let cross =
            (welded[mapped[1]] - welded[mapped[0]]).cross(welded[mapped[2]] - welded[mapped[0]]);
        if cross.dot(cross) < 0.000001 {
            continue;
        }
        let mut canonical = mapped;
        canonical.sort_unstable();
        if seen.insert(canonical) {
            triangles.push(mapped);
        }
    }

    let mut used = vec![false; welded.len()];
    for index in triangles.iter().flatten() {
        used[*index] = true;
    }
    let mut compact = Vec::new();
    let mut compact_map = vec![0; welded.len()];
    for (index, vertex) in welded.into_iter().enumerate() {
        if used[index] {
            compact_map[index] = compact.len();
            compact.push(vertex);
        }
    }
    for face in &mut triangles {
        *face = face.map(|index| compact_map[index]);
    }
    mesh.vertices = compact;
    mesh.triangles = triangles;
    (
        old_vertices.saturating_sub(mesh.vertices.len()),
        old_triangles.saturating_sub(mesh.triangles.len()),
    )
}
