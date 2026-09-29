# Contribuir

Gracias por el interés. Esto es un proyecto chico y bastante opinado (estética de macOS, Rust nativo, sin
dependencias que no hagan falta), así que antes de un cambio grande conviene abrir un *issue* para
charlarlo — evita trabajo perdido de los dos lados.

## Antes que nada

Leé [`docs/ARQUITECTURA.md`](docs/ARQUITECTURA.md): explica qué archivo controla cada panel, cómo ver qué
hace cada acción contra Windows y cómo agregar un panel o un ajuste del catálogo sin tocar código.

## Poner el entorno en marcha

Requisitos: Windows 10/11, [Rust](https://rustup.rs) 1.85+ y las *Build Tools* de Visual Studio con el SDK
de Windows (ver [Desarrollo](README.md#desarrollo) en el README). Si el build se queda sin memoria,
[`AGENTS.md`](AGENTS.md) tiene los ajustes para compilar con poca RAM.

```bash
cargo run -p liquidcontrol                     # la app en modo desarrollo
cargo test --workspace                   # pruebas (incluye la validación del catálogo)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

## Antes de abrir el PR

- `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` y `cargo test --workspace`
  sin errores ni avisos — es exactamente lo que corre la CI (`.github/workflows/ci.yml`).
- Si el cambio se ve en pantalla, una captura en la descripción del PR ayuda mucho a revisarlo rápido.
- Sumá una línea a `CHANGELOG.md` bajo `## [Sin publicar]` si el cambio es visible para quien usa la app.
- Los comentarios y los mensajes de commit van en español rioplatense, como el resto del proyecto.

## Reglas que no son negociables

- **Nada de credenciales ni telemetría propia.** La app no manda datos a ningún lado salvo lo que ya está
  documentado (íconos de apps, búsqueda de fondos, AirDrop en la red local) — todo opcional y explicado en
  el README.
- **Sin shell arbitrario.** Los procesos se lanzan por ruta absoluta con argumentos separados; nada de
  concatenar strings para armar un comando.
- **Presupuesto de RAM: ≤100 MB en uso normal.** Medilo con el `.exe` de verdad (Administrador de tareas o
  `Get-Process liquidcontrol`), no lo asumas. `docs/ARQUITECTURA.md` explica por qué algunos componentes de la
  interfaz no pueden aceptar hijos (`@children`): Slint duplica ese código en cada uso y puede hacer que el
  build de release se quede sin memoria.
- **Sin código copiado de otro proyecto sin una licencia compatible con GPLv3.** Ver
  [Créditos y dependencias](README.md#créditos-y-dependencias) para cómo se documentó lo que sí se usó como
  referencia.

## Reportar un problema

Un *issue* con qué esperabas, qué pasó, y los pasos para reproducirlo alcanza. Si es algo que se ve en
pantalla, una captura ayuda. Si es sobre un ajuste puntual, el botón **Detalles** de la app muestra las
operaciones exactas que hace — copiarlas en el *issue* ahorra una vuelta.
