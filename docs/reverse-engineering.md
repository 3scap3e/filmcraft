# Reverse Engineering

RE1 adds a native dockable panel shell. Open it with **Window → Reverse Engineering**.
It uses the existing docking, close, float and workspace-layout persistence behavior.
Opening or closing it does not edit the media project. Analysis tools are not implemented in RE1.

Automation uses the existing UI commands:

```json
{"method":"ui.panel.show","params":{"panel":"ReverseEngineering"}}
{"method":"ui.panel.close","params":{"panel":"ReverseEngineering"}}
```

The Window menu command is `window.panel.ReverseEngineering`. The introductory label
is exposed as `reverseEngineering.intro` through `ui.elements`.

Focused verification: `cargo test --release -p filmcraft-ui-egui --test panels_ui reverse_engineering`.
