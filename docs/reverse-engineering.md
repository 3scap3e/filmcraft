# Reverse Engineering

RE1 adds a native dockable panel shell. It opens by default beside **Metadata** in Editing, Assembly and All Panels. You can also open it with **Window → Reverse Engineering**.

Existing saved layouts gain the new tab once. After that migration, closing the panel and saving your layout remains respected.
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
