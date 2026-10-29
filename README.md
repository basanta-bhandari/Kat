# ForgeTUI

ForgeTUI is a lightweight terminal CAD tool focused on preparing solid models for 3D printing. It works in millimetres, uses a Z-up build plate, and renders a real anti-aliased RGB viewport through Kitty graphics or Sixel. The surrounding interface remains a low-power, mouse-enabled TUI.

> Status: functional prototype. ForgeTUI is a mesh modeller, not yet a parametric sketch/B-Rep replacement for production CAD.

## Build and run

Requires Rust 1.88 or newer.

```bash
cargo build --release --locked
./target/release/forgetui
```

Open an existing printable model or the demo:

```bash
./target/release/forgetui bracket.3mf
./target/release/forgetui part.stl
./target/release/forgetui --demo
```

Graphics are detected automatically. In Kitty, use `:graphics kitty` if detection is unavailable. Sixel and a plain TTY fallback are also supported; the main experience is the pixel viewport.

Without a filename, the app opens the project browser. Click a folder or project, press `n` for a new project, use arrow keys and Enter to navigate, and `/` to filter the current folder. Explicit filenames and `--demo` open the workspace directly.

On slower machines, `FORGETUI_QUALITY=low ./target/release/forgetui` reduces viewport resolution and disables supersampling. Rendering is software-based and does not require a dedicated graphics card or Mesa GPU acceleration. Idle redraws are event-driven.

## Print workflow

Press `a` to add a solid, click or right-click it to work with it, and use plain commands for exact operations:

```text
size 20 30 5        exact X/Y/Z dimensions in mm
size z 12           exact height
add tube 30 12 8    tube: outside diameter, bore diameter, height
array x 4 25        four objects spaced 25 mm apart
move x 10
place               put the object on the bed
center              centre it and put it on the bed
check                validate printable geometry
repair               weld duplicates and remove bad triangles
bed 220 220 250      set printer build volume
export bracket.3mf
slice                export 3MF and launch an installed slicer
```

The **Print** menu exposes the same actions without memorising commands. Slicer handoff detects OrcaSlicer, PrusaSlicer, Bambu Studio, or Cura on `PATH`.

Use **Update → Check for updates** to compare with the GitHub main branch over SSH. **Update → Install from GitHub** clones the repository into a temporary directory, builds with Cargo, and replaces the current executable after a successful build. It requires Git, Rust/Cargo, working GitHub SSH access, and permission to write the executable; restart after installation. It does not modify the current source checkout.

Saving uses an in-app **Save As** window: browse folders, edit the name, choose `.forge`, `.3mf`, `.stl`, or `.step`, then click Save. Existing files require a second confirmation. `Ctrl-S` saves the current project, `Ctrl-Shift-S` opens Save As, and `E` opens the same window with 3MF selected for quick print export.

Mouse tools directly manipulate the selected object. A click only selects; it does not open a command prompt. With Move selected, left-drag moves in the visible screen plane and right-drag changes Z height. Rotate uses left-drag for X/Z and right-drag for Y/Z. Scale uses left-drag uniformly and right-drag for Z only. A right-click without movement still opens the object menu, while right- or middle-drag pans when the Select tool is active.

## File formats

| Format | Support |
|---|---|
| `.forge` | Native editable project |
| `.stl` | Built-in binary/ASCII import and binary export |
| `.3mf` | Built-in import/export in millimetres |
| `.step` / `.stp` | Optional import/export through `FreeCADCmd` |

The former Fusion 360, Onshape, Blender, OBJ, and general-renderer ambitions are intentionally outside the product scope.

## Current features

- 220 × 220 × 250 mm default build volume and 10 mm bed grid
- Cube, sphere, cylinder, cone, torus, wedge, tube, and construction plane
- Dimensioned tube creation and linear object patterns
- Exact dimensions, move/rotate/scale, duplicate, mirror, snap, and undo/redo
- Watertightness, degenerate-face, non-manifold-edge, bed-fit, bounds, volume, and overhang checks
- Basic mesh repair, centre-on-bed, and place-on-bed tools
- Click selection, orbit, pan, zoom, right-click actions, side toolbar, and top menus
- Solid, wireframe, and X-ray views; perspective/orthographic cameras
- Kitty/Sixel RGB output with lighting and anti-aliasing
- Five themes and event-driven redraws for low idle CPU use

## Controls

| Input | Action |
|---|---|
| Select tool: left-drag / right- or middle-drag | Orbit / pan |
| Move tool: left-drag / right-drag | Screen-plane move / Z move |
| Rotate tool: left-drag / right-drag | X/Z rotate / Y/Z rotate |
| Scale tool: left-drag / right-drag | Uniform scale / Z scale |
| Right-click without dragging | Object actions |
| Wheel | Zoom |
| `a`, `g`, `r`, `s` | Add, move, rotate, scale |
| `e`, `o` | Export, open |
| `Ctrl-S`, `Ctrl-Shift-S` | Save, Save As |
| `1`, `3`, `7`, `0` | Front, side, top, isometric |
| `5` | Perspective/orthographic |
| `t`, `v` | Cycle theme, graphics mode |
| `:` | Enter a command |
| `?` | Built-in help |
| `q` | Quit |

Use `./target/release/forgetui --formats` for the format summary and `--render FILE` for a headless terminal preview.

This remains an early mesh editor: it does not provide parametric sketches, constraints, boolean operations, or a full feature history. Exact `size` commands operate on unrotated objects; size them before rotating. Basic repair cannot fill complex holes. 3MF import currently supports direct millimetre meshes; flatten component assemblies and transforms before importing. Check the resulting model in your slicer before printing.
