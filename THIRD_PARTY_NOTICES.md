# Avisos de terceros

LiquidControl se distribuye bajo GPL-3.0-only. Los siguientes componentes conservan sus respectivas licencias:

- [Slint](https://slint.dev): GPL-3.0-only en esta distribución.
- [Lucide](https://lucide.dev): ISC. La copia de la licencia está en `crates/app/ui/icons/LICENSE`.
- [`windows` y `windows-core`](https://github.com/microsoft/windows-rs): MIT o Apache-2.0.
- `serde`, `serde_json`, `thiserror`, `base64`, `log` y `qrcode`: MIT o Apache-2.0.
- `winreg`: MIT.
- La lectura y escritura del formato no documentado de Luz nocturna se basa en `NightLight.cs` de
  [tiny-screen](https://github.com/Maclay74/tiny-screen), publicado bajo MIT, y en su adaptación en
  [nightlight-cli](https://github.com/nathanbabcock/nightlight-cli).
- El catálogo de ajustes (`crates/core/catalog/tweaks.json`) se contrastó con el inventario de ajustes de 
  [WinScript](https://github.com/flick9000/winscript) (GPL-3.0) para no dejar afuera ninguno: las claves de registro,
  servicios y tareas son datos del propio Windows, y la lógica de activar, revertir y restaurar es propia de
  LiquidControl.
- La compatibilidad de transferencia local implementa de forma independiente la
  [especificación del protocolo LocalSend v2](https://github.com/localsend/protocol). No incluye código de la
  aplicación LocalSend.

El árbol exacto de dependencias y versiones de cada compilación queda fijado en `Cargo.lock`. Los textos de licencia
de las dependencias están disponibles en sus paquetes fuente de crates.io.
