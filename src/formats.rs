use crate::{
    math::Vec3,
    scene::{Mesh, Scene},
};
use flate2::read::DeflateDecoder;
use std::{
    collections::HashMap,
    error::Error,
    fmt, fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug)]
pub struct FormatError(pub String);
impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for FormatError {}
pub type Result<T> = std::result::Result<T, FormatError>;

pub fn save(scene: &Scene, path: &Path) -> Result<()> {
    match extension(path).as_str() {
        "forge" => save_native(scene, path),
        "stl" => export_stl(scene, path),
        "3mf" => export_3mf(scene, path),
        "step" | "stp" => export_step_via_freecad(scene, path),
        ext => Err(FormatError(format!(
            "unknown output format '.{ext}'; use .forge, .stl, .3mf, or optional .step"
        ))),
    }
}

pub fn load(path: &Path) -> Result<Scene> {
    let scene = match extension(path).as_str() {
        "forge" => load_native(path),
        "stl" => import_stl(path),
        "3mf" => import_3mf(path),
        "step" | "stp" => import_via_freecad(path),
        ext => Err(FormatError(format!(
            "unknown input format '.{ext}'; use .forge, .stl, .3mf, or optional .step"
        ))),
    }?;
    validate_scene(&scene)?;
    Ok(scene)
}

fn validate_scene(scene: &Scene) -> Result<()> {
    for object in &scene.objects {
        if object
            .mesh
            .vertices
            .iter()
            .any(|vertex| !vertex.x.is_finite() || !vertex.y.is_finite() || !vertex.z.is_finite())
        {
            return Err(FormatError(format!(
                "{} has non-finite vertex coordinates",
                object.name
            )));
        }
        if object
            .mesh
            .triangles
            .iter()
            .flatten()
            .any(|&index| index >= object.mesh.vertices.len())
        {
            return Err(FormatError(format!(
                "{} has an out-of-range triangle index",
                object.name
            )));
        }
    }
    Ok(())
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}
fn save_native(scene: &Scene, path: &Path) -> Result<()> {
    let data = serde_json::to_string_pretty(scene).map_err(|e| FormatError(e.to_string()))?;
    fs::write(path, data)
        .map_err(|e| FormatError(format!("could not save {}: {e}", path.display())))
}
fn load_native(path: &Path) -> Result<Scene> {
    let data = fs::read_to_string(path)
        .map_err(|e| FormatError(format!("could not open {}: {e}", path.display())))?;
    let mut scene: Scene = serde_json::from_str(&data)
        .map_err(|e| FormatError(format!("invalid ForgeTUI project: {e}")))?;
    scene.repair_after_load();
    Ok(scene)
}

pub fn export_stl(scene: &Scene, path: &Path) -> Result<()> {
    let triangles: Vec<_> = scene
        .world_triangles()
        .map(|(_, _, triangle)| triangle)
        .collect();
    let mut out = vec![0; 80];
    let header = b"ForgeTUI binary STL (mm)";
    out[..header.len()].copy_from_slice(header);
    out.extend_from_slice(&(triangles.len() as u32).to_le_bytes());
    for triangle in triangles {
        let normal = (triangle[1] - triangle[0])
            .cross(triangle[2] - triangle[0])
            .normalized();
        for value in [normal, triangle[0], triangle[1], triangle[2]] {
            out.extend_from_slice(&value.x.to_le_bytes());
            out.extend_from_slice(&value.y.to_le_bytes());
            out.extend_from_slice(&value.z.to_le_bytes());
        }
        out.extend_from_slice(&0_u16.to_le_bytes());
    }
    fs::write(path, out)
        .map_err(|e| FormatError(format!("could not export {}: {e}", path.display())))
}

pub fn import_stl(path: &Path) -> Result<Scene> {
    let bytes = fs::read(path)
        .map_err(|e| FormatError(format!("could not open {}: {e}", path.display())))?;
    let mesh = if bytes.len() >= 84 {
        let count = u32_at(&bytes, 80).unwrap_or(0) as usize;
        if count
            .checked_mul(50)
            .and_then(|n| n.checked_add(84))
            .is_some_and(|n| n <= bytes.len())
        {
            parse_binary_stl(&bytes, count)?
        } else {
            parse_ascii_stl(&bytes)?
        }
    } else {
        parse_ascii_stl(&bytes)?
    };
    scene_from_mesh(path, mesh)
}

fn parse_binary_stl(bytes: &[u8], count: usize) -> Result<Mesh> {
    let mut vertices = Vec::new();
    let mut triangles = Vec::with_capacity(count);
    let mut lookup = HashMap::new();
    for triangle_index in 0..count {
        let start = 84 + triangle_index * 50 + 12;
        let mut face = [0; 3];
        for (point_index, output_index) in face.iter_mut().enumerate() {
            let offset = start + point_index * 12;
            let point = Vec3::new(
                f32_at(bytes, offset)?,
                f32_at(bytes, offset + 4)?,
                f32_at(bytes, offset + 8)?,
            );
            let key = (point.x.to_bits(), point.y.to_bits(), point.z.to_bits());
            *output_index = *lookup.entry(key).or_insert_with(|| {
                vertices.push(point);
                vertices.len() - 1
            });
        }
        triangles.push(face);
    }
    Ok(Mesh {
        vertices,
        triangles,
    })
}

fn parse_ascii_stl(bytes: &[u8]) -> Result<Mesh> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| FormatError("STL is neither valid binary nor ASCII".into()))?;
    let mut vertices = Vec::new();
    let mut triangles = Vec::new();
    let mut pending = Vec::new();
    let mut lookup = HashMap::new();
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("vertex") {
            let values: Vec<f32> = rest
                .split_whitespace()
                .map(str::parse)
                .collect::<std::result::Result<_, _>>()
                .map_err(|_| FormatError("invalid ASCII STL vertex".into()))?;
            if values.len() != 3 {
                return Err(FormatError(
                    "ASCII STL vertex needs three coordinates".into(),
                ));
            }
            let point = Vec3::new(values[0], values[1], values[2]);
            let key = (point.x.to_bits(), point.y.to_bits(), point.z.to_bits());
            pending.push(*lookup.entry(key).or_insert_with(|| {
                vertices.push(point);
                vertices.len() - 1
            }));
            if pending.len() == 3 {
                triangles.push([pending[0], pending[1], pending[2]]);
                pending.clear();
            }
        }
    }
    if triangles.is_empty() {
        return Err(FormatError("STL contains no triangles".into()));
    }
    Ok(Mesh {
        vertices,
        triangles,
    })
}

pub fn export_3mf(scene: &Scene, path: &Path) -> Result<()> {
    let mut model = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\">\n <metadata name=\"Application\">ForgeTUI</metadata>\n <resources>\n",
    );
    let mut ids = Vec::new();
    for (index, object) in scene
        .objects
        .iter()
        .filter(|o| o.visible && !o.mesh.triangles.is_empty())
        .enumerate()
    {
        let id = index + 1;
        ids.push(id);
        model.push_str(&format!(
            "  <object id=\"{id}\" type=\"model\" name=\"{}\"><mesh><vertices>\n",
            xml_escape(&object.name)
        ));
        for vertex in &object.mesh.vertices {
            let p = object.transform.point(*vertex);
            model.push_str(&format!(
                "   <vertex x=\"{}\" y=\"{}\" z=\"{}\"/>\n",
                p.x, p.y, p.z
            ));
        }
        model.push_str("  </vertices><triangles>\n");
        for [a, b, c] in &object.mesh.triangles {
            model.push_str(&format!(
                "   <triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"/>\n"
            ));
        }
        model.push_str("  </triangles></mesh></object>\n");
    }
    model.push_str(" </resources>\n <build>\n");
    for id in ids {
        model.push_str(&format!("  <item objectid=\"{id}\"/>\n"));
    }
    model.push_str(" </build>\n</model>\n");
    let types = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/></Types>";
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Target=\"/3D/3dmodel.model\" Id=\"rel0\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/></Relationships>";
    fs::write(
        path,
        zip_store(&[
            ("[Content_Types].xml", types.as_bytes()),
            ("_rels/.rels", rels.as_bytes()),
            ("3D/3dmodel.model", model.as_bytes()),
        ]),
    )
    .map_err(|e| FormatError(format!("could not export {}: {e}", path.display())))
}

pub fn import_3mf(path: &Path) -> Result<Scene> {
    let archive = fs::read(path)
        .map_err(|e| FormatError(format!("could not open {}: {e}", path.display())))?;
    let model = zip_member(&archive, "3D/3dmodel.model")?;
    let xml =
        String::from_utf8(model).map_err(|_| FormatError("3MF model XML is not UTF-8".into()))?;
    let model_tag = xml
        .find("<model")
        .and_then(|start| xml[start..].find('>').map(|end| &xml[start..start + end]))
        .ok_or_else(|| FormatError("3MF has no model tag".into()))?;
    if xml_attr(model_tag, "unit").is_some_and(|unit| unit != "millimeter") {
        return Err(FormatError(
            "3MF units other than millimeters are not yet supported".into(),
        ));
    }
    if xml.contains("<components") || xml.contains(" transform=") {
        return Err(FormatError(
            "3MF components and build transforms are not yet supported; export a flattened mesh"
                .into(),
        ));
    }
    let mut scene = empty_scene(path);
    let mut remaining = xml.as_str();
    while let Some(start) = remaining.find("<object ") {
        remaining = &remaining[start..];
        let Some(end) = remaining.find("</object>") else {
            break;
        };
        let object_xml = &remaining[..end];
        let header_end = object_xml.find('>').unwrap_or(0);
        let name =
            xml_attr(&object_xml[..header_end], "name").unwrap_or_else(|| "3MF object".into());
        let mut vertices = Vec::new();
        let mut cursor = object_xml;
        while let Some(position) = cursor.find("<vertex ") {
            cursor = &cursor[position..];
            let Some(tag_end) = cursor.find('>') else {
                break;
            };
            let tag = &cursor[..tag_end];
            vertices.push(Vec3::new(
                xml_number(tag, "x")?,
                xml_number(tag, "y")?,
                xml_number(tag, "z")?,
            ));
            cursor = &cursor[tag_end + 1..];
        }
        let mut triangles = Vec::new();
        cursor = object_xml;
        while let Some(position) = cursor.find("<triangle ") {
            cursor = &cursor[position..];
            let Some(tag_end) = cursor.find('>') else {
                break;
            };
            let tag = &cursor[..tag_end];
            triangles.push([
                xml_index(tag, "v1")?,
                xml_index(tag, "v2")?,
                xml_index(tag, "v3")?,
            ]);
            cursor = &cursor[tag_end + 1..];
        }
        if !vertices.is_empty() && !triangles.is_empty() {
            scene.add_mesh(
                name,
                Mesh {
                    vertices,
                    triangles,
                },
            );
        }
        remaining = &remaining[end + "</object>".len()..];
    }
    if scene.objects.is_empty() {
        Err(FormatError(
            "3MF contains no directly embedded triangle meshes".into(),
        ))
    } else {
        Ok(scene)
    }
}

pub fn open_in_slicer(scene: &Scene) -> Result<String> {
    let path = std::env::temp_dir().join(format!("forgetui-slice-{}.3mf", std::process::id()));
    export_3mf(scene, &path)?;
    for command in [
        "orca-slicer",
        "OrcaSlicer",
        "prusa-slicer",
        "bambu-studio",
        "cura",
    ] {
        if command_exists(command) {
            Command::new(command)
                .arg(&path)
                .spawn()
                .map_err(|e| FormatError(format!("could not start {command}: {e}")))?;
            return Ok(format!("Opened {} in {command}", path.display()));
        }
    }
    Err(FormatError(format!(
        "no supported slicer found on PATH; 3MF prepared at {}",
        path.display()
    )))
}

fn scene_from_mesh(path: &Path, mesh: Mesh) -> Result<Scene> {
    if mesh.vertices.is_empty() || mesh.triangles.is_empty() {
        return Err(FormatError("mesh contains no renderable triangles".into()));
    }
    let mut scene = empty_scene(path);
    scene.add_mesh(scene.name.clone(), mesh);
    Ok(scene)
}
fn empty_scene(path: &Path) -> Scene {
    let mut scene = Scene::default();
    scene.objects.clear();
    scene.name = path
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("Imported")
        .into();
    scene
}
fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}
fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}
fn f32_at(bytes: &[u8], offset: usize) -> Result<f32> {
    Ok(f32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(|| FormatError("truncated binary STL".into()))?
            .try_into()
            .map_err(|_| FormatError("truncated binary STL".into()))?,
    ))
}
fn put16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn put32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn zip_store(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut entries = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        put32(&mut out, 0x0403_4b50);
        for value in [20, 0, 0, 0, 0] {
            put16(&mut out, value);
        }
        put32(&mut out, crc);
        put32(&mut out, data.len() as u32);
        put32(&mut out, data.len() as u32);
        put16(&mut out, name.len() as u16);
        put16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        entries.push((*name, data.len() as u32, crc, offset));
    }
    let directory_offset = out.len() as u32;
    for (name, size, crc, offset) in &entries {
        put32(&mut out, 0x0201_4b50);
        put16(&mut out, 20);
        put16(&mut out, 20);
        for value in [0, 0, 0, 0] {
            put16(&mut out, value);
        }
        put32(&mut out, *crc);
        put32(&mut out, *size);
        put32(&mut out, *size);
        put16(&mut out, name.len() as u16);
        for _ in 0..4 {
            put16(&mut out, 0);
        }
        put32(&mut out, 0);
        put32(&mut out, *offset);
        out.extend_from_slice(name.as_bytes());
    }
    let directory_size = out.len() as u32 - directory_offset;
    put32(&mut out, 0x0605_4b50);
    put16(&mut out, 0);
    put16(&mut out, 0);
    put16(&mut out, entries.len() as u16);
    put16(&mut out, entries.len() as u16);
    put32(&mut out, directory_size);
    put32(&mut out, directory_offset);
    put16(&mut out, 0);
    out
}

fn zip_member(archive: &[u8], wanted: &str) -> Result<Vec<u8>> {
    let eocd = archive
        .windows(4)
        .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
        .ok_or_else(|| FormatError("invalid 3MF ZIP directory".into()))?;
    let entries = u16_at(archive, eocd + 10).ok_or_else(|| FormatError("truncated 3MF".into()))?;
    let mut cursor =
        u32_at(archive, eocd + 16).ok_or_else(|| FormatError("truncated 3MF".into()))? as usize;
    for _ in 0..entries {
        if u32_at(archive, cursor) != Some(0x0201_4b50) {
            return Err(FormatError("invalid 3MF ZIP entry".into()));
        }
        let method = u16_at(archive, cursor + 10).unwrap_or(0);
        let size = u32_at(archive, cursor + 20).unwrap_or(0) as usize;
        let name_len = u16_at(archive, cursor + 28).unwrap_or(0) as usize;
        let extra = u16_at(archive, cursor + 30).unwrap_or(0) as usize;
        let comment = u16_at(archive, cursor + 32).unwrap_or(0) as usize;
        let local = u32_at(archive, cursor + 42).unwrap_or(0) as usize;
        let name = std::str::from_utf8(
            archive
                .get(cursor + 46..cursor + 46 + name_len)
                .ok_or_else(|| FormatError("truncated 3MF ZIP name".into()))?,
        )
        .unwrap_or_default();
        if name.eq_ignore_ascii_case(wanted) {
            let local_name = u16_at(archive, local + 26).unwrap_or(0) as usize;
            let local_extra = u16_at(archive, local + 28).unwrap_or(0) as usize;
            let start = local + 30 + local_name + local_extra;
            let data = archive
                .get(start..start + size)
                .ok_or_else(|| FormatError("truncated 3MF ZIP data".into()))?;
            return match method {
                0 => Ok(data.to_vec()),
                8 => {
                    let mut decoded = Vec::new();
                    DeflateDecoder::new(data)
                        .read_to_end(&mut decoded)
                        .map_err(|e| FormatError(format!("could not decompress 3MF: {e}")))?;
                    Ok(decoded)
                }
                _ => Err(FormatError(format!(
                    "unsupported 3MF ZIP compression method {method}"
                ))),
            };
        }
        cursor += 46 + name_len + extra + comment;
    }
    Err(FormatError("3MF has no 3D/3dmodel.model part".into()))
}

fn xml_attr(tag: &str, name: &str) -> Option<String> {
    let marker = format!("{name}=\"");
    let start = tag.find(&marker)? + marker.len();
    let end = tag[start..].find('"')? + start;
    Some(
        tag[start..end]
            .replace("&quot;", "\"")
            .replace("&amp;", "&"),
    )
}
fn xml_number(tag: &str, name: &str) -> Result<f32> {
    xml_attr(tag, name)
        .ok_or_else(|| FormatError(format!("3MF vertex is missing {name}")))?
        .parse()
        .map_err(|_| FormatError(format!("invalid 3MF {name} coordinate")))
}
fn xml_index(tag: &str, name: &str) -> Result<usize> {
    xml_attr(tag, name)
        .ok_or_else(|| FormatError(format!("3MF triangle is missing {name}")))?
        .parse()
        .map_err(|_| FormatError(format!("invalid 3MF {name} index")))
}
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}
fn command_exists(name: &str) -> bool {
    Command::new(name).arg("--version").output().is_ok()
}
fn temporary_obj(path: &Path) -> PathBuf {
    std::env::temp_dir().join(format!(
        "forgetui-{}-{}.obj",
        path.file_stem().and_then(|v| v.to_str()).unwrap_or("scene"),
        std::process::id()
    ))
}

fn export_obj(scene: &Scene, path: &Path) -> Result<()> {
    let mut out = String::from("# ForgeTUI STEP bridge\n");
    let mut offset = 1;
    for object in scene.objects.iter().filter(|o| o.visible) {
        for vertex in &object.mesh.vertices {
            let p = object.transform.point(*vertex);
            out.push_str(&format!("v {} {} {}\n", p.x, p.y, p.z));
        }
        for [a, b, c] in &object.mesh.triangles {
            out.push_str(&format!("f {} {} {}\n", a + offset, b + offset, c + offset));
        }
        offset += object.mesh.vertices.len();
    }
    fs::write(path, out).map_err(|e| FormatError(e.to_string()))
}
fn import_obj(path: &Path) -> Result<Scene> {
    let input = fs::read_to_string(path).map_err(|e| FormatError(e.to_string()))?;
    let mut vertices = Vec::new();
    let mut triangles = Vec::new();
    for line in input.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("v ") {
            let v: Vec<f32> = rest
                .split_whitespace()
                .take(3)
                .map(str::parse)
                .collect::<std::result::Result<_, _>>()
                .map_err(|_| FormatError("invalid FreeCAD OBJ vertex".into()))?;
            if v.len() == 3 {
                vertices.push(Vec3::new(v[0], v[1], v[2]));
            }
        } else if let Some(rest) = line.strip_prefix("f ") {
            let indices: Vec<usize> = rest
                .split_whitespace()
                .filter_map(|token| {
                    token
                        .split('/')
                        .next()?
                        .parse::<usize>()
                        .ok()
                        .map(|i| i - 1)
                })
                .collect();
            for i in 1..indices.len().saturating_sub(1) {
                triangles.push([indices[0], indices[i], indices[i + 1]]);
            }
        }
    }
    scene_from_mesh(
        path,
        Mesh {
            vertices,
            triangles,
        },
    )
}
fn freecad_command() -> Option<&'static str> {
    ["FreeCADCmd", "freecadcmd"]
        .into_iter()
        .find(|name| command_exists(name))
}
fn export_step_via_freecad(scene: &Scene, path: &Path) -> Result<()> {
    let Some(freecad) = freecad_command() else {
        return Err(FormatError(
            "STEP export is optional and needs FreeCADCmd on PATH; use .3mf or .stl now".into(),
        ));
    };
    let temp = temporary_obj(path);
    export_obj(scene, &temp)?;
    let script = std::env::temp_dir().join(format!("forgetui-step-{}.py", std::process::id()));
    let source = format!(
        "import FreeCAD as App, Mesh, Part\ndoc=App.newDocument('ForgeTUI')\nmesh=Mesh.Mesh({:?})\nshape=Part.Shape()\nshape.makeShapeFromMesh(mesh.Topology, 0.05)\nobj=doc.addObject('Part::Feature', 'Model')\nobj.Shape=shape.removeSplitter()\ndoc.recompute()\nPart.export([obj], {:?})\n",
        temp.to_string_lossy(),
        path.to_string_lossy()
    );
    fs::write(&script, source).map_err(|e| FormatError(e.to_string()))?;
    let status = Command::new(freecad).arg(&script).status();
    let _ = fs::remove_file(&temp);
    let _ = fs::remove_file(&script);
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(FormatError(format!(
            "FreeCAD STEP conversion failed with {s}"
        ))),
        Err(e) => Err(FormatError(format!("could not start FreeCAD: {e}"))),
    }
}
fn import_via_freecad(path: &Path) -> Result<Scene> {
    let Some(freecad) = freecad_command() else {
        return Err(FormatError(
            "STEP import is optional and needs FreeCADCmd on PATH".into(),
        ));
    };
    let temp = temporary_obj(path);
    let script = std::env::temp_dir().join(format!("forgetui-import-{}.py", std::process::id()));
    let source = format!(
        "import Part, Mesh\nshape=Part.read({:?})\nvertices, facets=shape.tessellate(0.1)\nmesh=Mesh.Mesh((vertices, facets))\nmesh.write({:?})\n",
        path.to_string_lossy(),
        temp.to_string_lossy()
    );
    fs::write(&script, source).map_err(|e| FormatError(e.to_string()))?;
    let status = Command::new(freecad)
        .arg(&script)
        .status()
        .map_err(|e| FormatError(e.to_string()))?;
    let _ = fs::remove_file(&script);
    if !status.success() {
        return Err(FormatError(format!(
            "FreeCAD STEP conversion failed with {status}"
        )));
    }
    let scene = import_obj(&temp);
    let _ = fs::remove_file(&temp);
    scene
}

pub fn format_matrix() -> &'static str {
    "Native project: .forge\n3D printing: .stl and .3mf import/export\nCAD interchange: .step/.stp through optional FreeCADCmd"
}
