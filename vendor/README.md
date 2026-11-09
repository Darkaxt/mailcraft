# Retained window-library source

`winit/` retains MailCraft's existing winit0.30.13 dependency at
UCS-Care-Group/winit@951b6c131897a80d27a925af7c5da22ea522eb0d. This keeps its
existing Wayland and activation-token fixes while adding the Windows caption
hit-test API needed by MailCraft's integrated header. No new GUI framework,
desktop controller, process or system service is involved.

The narrow patch publishes client rectangles through a safe Windows extension.
The existing native window procedure owns resize/caption hit testing and Windows
Snap Layout behavior. MailCraft never manufactures HWNDs or installs a subclass.
Original native interop remains inside the dependency's existing implementation;
new application and caption-layout code uses safe Rust. Other platforms preserve
their existing paths. Upstream tests are retained as dependency source; new patch
regressions live in their own test file.

`winit/sources.json` records every imported file's original source hash and the
exact existing dependency revision. Rust files retain their original contents
with accurate Apache-2.0 SPDX attribution prepended. `winit/LICENSE` retains the
dependency licence. Final project verification must cover the Windows native
frame, high DPI, Snap Layouts, close guards and the unchanged non-Windows paths.
