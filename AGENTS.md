# Notas de compilación para agentes (y humanos)

Esta PC de desarrollo tiene poca RAM disponible para compilar: `.cargo/config.toml` y los perfiles de
`Cargo.toml` ya están ajustados para no quedarse sin memoria. No hace falta tocarlos para el uso normal.

## Qué hace cada ajuste

**`.cargo/config.toml`**
```toml
[build]
jobs = 2            # como mucho 2 crates compilando en paralelo
incremental = true

[env]
RUSTFLAGS = "-C target-feature=-crt-static"  # runtime de MSVC dinámico, igual que Skia
```
La última línea mantiene el runtime de MSVC dinámico, igual que Skia. Sin forzarlo, el enlazador final puede
fallar con `LNK2038`/`LNK1169` por el choque entre runtime estático y dinámico.

**`Cargo.toml`** (`[profile.dev]`, `[profile.release]`)
- **Dev:** `codegen-units = 256`, `opt-level = 0` — compila rápido y con poca memoria; las dependencias
  (`[profile.dev.package."*"]`) sí se optimizan (`opt-level = 2`), para que la interfaz responda fluida
  aunque el propio código de la app esté sin optimizar.
- **Release:** `codegen-units = 1`, `lto = "thin"`, `opt-level = 3`, `panic = "abort"`, `strip = true` en
  general; el paquete `liquidcontrol` (la interfaz, con mucho código generado por Slint) usa
  `codegen-units = 16` y `opt-level = "s"` (por tamaño) para no disparar la memoria del enlazador con LTO
  completo sobre ese código.

## Comandos

```bash
cargo build -p liquidcontrol                  # desarrollo
./target/debug/liquidcontrol.exe
cargo build --release -p liquidcontrol        # release, el que se distribuye
./target/release/liquidcontrol.exe
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

En Windows, si `liquidcontrol.exe` está corriendo, el build de release falla al no poder reemplazar el
ejecutable: cerrar el proceso primero (`Get-Process liquidcontrol | Stop-Process -Force` en PowerShell).

## Si igual se queda sin memoria al compilar

1. `.cargo/config.toml`: bajar `jobs` a 1.
2. `Cargo.toml` → `[profile.dev]`: subir `codegen-units` (por ejemplo a 512).
3. Aumentar el archivo de paginación de Windows (Sistema → Almacenamiento → Almacenamiento adicional →
   tamaño del archivo de paginación; 1.5× la RAM física es un punto de partida razonable).

## Verificar de verdad, no solo que compile

La app apunta a **≤100 MB de RAM en uso** (`README.md`, sección "Nativo y liviano"). Un build limpio de
release en esta PC tarda entre 2 y 4 minutos; el ejecutable mide entre 25 y 30 MB. Antes de dar algo por
terminado, correr el `.exe` de verdad y medir con Administrador de tareas o `Get-Process liquidcontrol` — nunca
asumir el uso de memoria sin medirlo.
