# Arquitectura: dónde está cada cosa y cómo cambiarla

Esta guía responde tres preguntas: **qué archivo controla cada panel**, **cómo ver qué hace cada acción en el
sistema** y **cómo modificarla**.

## Las tres capas

```
ui/panes/<panel>.slint  ──callbacks──▶  src/panes/<panel>.rs  ──llama──▶  crates/core/src/<área>.rs  ──▶  Windows
   (cómo se ve)                          (controlador)                      (qué se cambia y cómo)
```

- **Interfaz** (`crates/app/ui`): solo dibuja. Cada panel tiene un `global <Panel>State` con propiedades (lo que se
  muestra) y callbacks (lo que pide el usuario). No toca el sistema.
- **Controladores** (`crates/app/src/panes`): conectan los callbacks, llaman al core en un hilo aparte
  (`bridge::spawn`) y actualizan la interfaz con el resultado. Cada archivo declara su `PANE` y
  `src/panes/mod.rs` los reúne en una lista: agregar un panel es sumar una línea.
- **Core** (`crates/core`): la única capa que habla con Windows (registro, COM, WinRT, Win32). Una función por
  acción, por ejemplo `power::delete_plan` o `system::remove_startup_item`. Tiene pruebas y no depende de la
  interfaz.

## Mapa de paneles

| Panel | Interfaz | Controlador | Core (qué toca en Windows) |
| --- | --- | --- | --- |
| Cuenta de Windows | `panes/account.slint` | `panes/account.rs`, `account.rs` | `system::account` (nombre, foto, tipo de cuenta) |
| ↳ iCloud | `panes/icloud.slint` | `panes/icloud.rs` | `icloud.rs`: detecta la app, carpetas, fotos (atributos de nube) |
| ↳ Fotos de iCloud (Ver todas) | `panes/icloud.slint` (`PhotosAllPage`) | `panes/icloud.rs` | `icloud.rs` + `photo_search.rs`: buscar por nombre, fecha o color/luz de la miniatura |
| Wi‑Fi | `panes/wifi.slint` | `panes/wifi.rs` | `wifi.rs`: API WLAN nativa (perfiles, conectar, radio) |
| AirDrop | `panes/airdrop.slint` | `panes/airdrop.rs` | `airdrop.rs`: servidor/cliente LocalSend v2 propio sobre `std::net` |
| ↳ iPhone y celulares | `panes/airdrop.slint` | `panes/airdrop.rs` | `airdrop/web.rs`: página para el celular (QR), sube/baja archivos |
| Bluetooth | `panes/bluetooth.slint` | `panes/bluetooth.rs` | `bluetooth.rs`: WinRT `Radios` y `DeviceInformation` |
| Red | `panes/network.slint` | `panes/network.rs` | `network.rs`: IP Helper (adaptadores) y DNS |
| Notificaciones | `panes/notifications.slint` | `panes/notifications.rs` | `notifications.rs`: `HKCU\…\Notifications\Settings` |
| Sonido | `panes/sound.slint` | `panes/sound.rs` | `sound.rs`: Core Audio (volumen, dispositivos; el predeterminado con `IPolicyConfig`) |
| ↳ Volumen por app | `panes/mixer.slint` | `panes/mixer.rs` | `mixer.rs`: sesiones de audio (`IAudioSessionManager2`) |
| General | `panes/general.slint` | `panes/general.rs` | `system.rs`: información, discos, ítems de inicio |
| Apariencia | `panes/appearance.slint` | `panes/appearance.rs` | `appearance.rs`: `Personalize`, `Accent`, `DWM` |
| Fondo de pantalla | `panes/wallpaper.slint` | `panes/wallpaper.rs` | `wallpaper.rs`: `IDesktopWallpaper`; búsqueda en línea en `wallpaper_search.rs` (junta `wallhaven.rs` y `openverse.rs`) |
| Accesibilidad | `panes/accessibility.slint` | `panes/accessibility.rs` | `accessibility.rs` (Lupa) + catálogo de ajustes |
| Escritorio / Privacidad | `panes/desktop.slint`, `privacy.slint` | `panes/tweaks.rs` | `tweaks.rs` + `catalog/tweaks.json` |
| Pantallas | `panes/displays.slint` | `panes/displays.rs` | `displays.rs`: modos, HDR (`DisplayConfig`), brillo DDC/CI |
| Energía | `panes/energy.slint` | `panes/energy.rs` | `power.rs`: API de planes de energía y `powercfg` |
| Optimización | `panes/optimize.slint` | `panes/optimize.rs` | `tweaks.rs`, `apps.rs` (`catalog/appx.json`) |
| Juegos | `panes/gaming.slint` | `panes/tweaks.rs` | `tweaks.rs` + `catalog/tweaks.json` (categoría `gaming`) |
| Aplicaciones | `panes/apps.slint` | `panes/apps.rs` | `apps.rs` (desinstaladores, winget), `icons.rs`, `net.rs` |
| ↳ Desinstalación profunda | `panes/apps.slint` | `panes/apps.rs` | `leftovers.rs`: busca restos, Papelera y `.reg` de respaldo |
| Mouse / Teclado | `panes/mouse.slint`, `keyboard.slint` | `panes/input.rs` | `input.rs`: `SystemParametersInfo` y registro |
| ↳ Idiomas y teclados | `panes/keyboard.slint` | `panes/languages.rs` | `keyboard.rs`: `Get/Set-WinUserLanguageList` (scripts fijos) |
| Ajustes de la app | `panes/app-settings.slint` | `panes/app_settings.rs` | `window.rs` (esquinas, tamaño según pantalla); preferencias en `src/prefs.rs` |
| ↳ Consola | `panes/console.slint` | `console.rs` | `activity.rs`: registro de actividad |

Las rutas de la interfaz son relativas a `crates/app/ui/`, las de los controladores a `crates/app/src/` y las del
core a `crates/core/src/`.

## Cómo ver qué hace cada acción

1. **En la app, antes de hacerlo:** cada ajuste del catálogo tiene un botón **Detalles** que muestra las operaciones
   exactas (qué clave del registro, qué valor, qué servicio o tarea programada).
2. **En la app, mientras pasa:** *Ajustes de la app → Consola de actividad* muestra en vivo cada escritura del
   registro, cada proceso que se ejecuta (con su resultado), cada cambio de servicio o tarea y cada error. Todo corre
   oculto: nunca se abre una ventana de consola aparte.
3. **Después:** el mismo registro queda en `data\logs\liquidcontrol.log` (se rota al llegar a 1 MB). Las contraseñas de
   Wi‑Fi nunca se anotan.

Por dentro, todo pasa por unas pocas funciones que dejan constancia de lo que hacen:

| Función | Anota en la consola como |
| --- | --- |
| `util/reg.rs` (`write_*`, `create_key`, `delete_*`) | `registro` |
| `util/process.rs` (`run`, `powershell`) | `procesos` |
| `util/services.rs`, `util/tasks.rs` | `servicios`, `tareas` |
| Cada módulo del core (`power.rs`, `system.rs`, …) | su área: `energía`, `inicio`, `apariencia`… |

## Cómo modificar una acción

### Ajustes del catálogo (Privacidad, Escritorio, Optimización, Accesibilidad)

Son datos, no código: se editan en `crates/core/catalog/tweaks.json`. Cada ajuste tiene una lista de operaciones:

```json
{
  "id": "desktop.show_extensions",
  "category": "desktop",
  "section": "Explorador de archivos",
  "title": "Mostrar las extensiones de archivo",
  "description": "Ves «foto.jpg» en lugar de «foto».",
  "risk": "safe",
  "restart": "explorer",
  "ops": [
    { "reg": "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced", "name": "HideFileExt", "type": "dword", "on": 0, "off": 1 }
  ]
}
```

- `reg` + `name` + `type` + `on`/`off`: valor del registro (`"off": null` borra el valor al desactivar).
- `regKey`: que exista o no una clave.
- `service` + `on`/`off`: tipo de inicio de un servicio (`auto`, `manual`, `disabled`).
- `task` + `on`: habilitar o deshabilitar una tarea programada.

Al activar un ajuste, el valor anterior se guarda en `data\journal.json`; al desactivarlo se restaura exactamente lo
que había. `cargo test` valida el catálogo completo (ids únicos, rutas y tipos correctos).

### Todo lo demás

Cada acción es una función del core con el mismo nombre que la acción. Para cambiar, por ejemplo, qué pasa al
quitar un ítem de inicio, se edita `system::remove_startup_item` en `crates/core/src/system.rs`; la interfaz y el
controlador no cambian.

### ¿Por qué el catálogo no se edita desde la app?

Porque LiquidControl a veces corre como administrador. Si leyera las acciones de un archivo que cualquier programa del
usuario puede modificar, un programa malicioso podría escribir ahí una clave del sistema y esperar a que LiquidControl la
aplique con permisos elevados. Por eso el catálogo va embebido en el `.exe` y se modifica en el código fuente; para
ver lo que hace sin tocar código están **Detalles** y la **Consola de actividad**.

## Agregar un panel

1. `ui/panes/mipanel.slint`: un `global MiPanelState` con propiedades y callbacks, y el componente del panel.
2. Exportar el global en `ui/app.slint`, mostrar el panel en `ui/shell/content.slint` y sumarlo al `Registry` de
   `ui/globals.slint` (título, ícono y color).
3. `src/panes/mipanel.rs` con `pub const PANE: Pane = Pane::new("mipanel", enter).with_init(init);` y sumarlo a
   `PANES` en `src/panes/mod.rs`. Si hace falta acceso al sistema, va en un módulo nuevo de `crates/core`.

## Reglas de la interfaz (tamaño del código generado)

Slint copia el código de un componente con `@children` en cada lugar donde se usa. Las filas (`Row`, `SliderRow`,
`PopupRow`, `SegmentedRow`, `SwatchRow`) se usan cientos de veces, así que **no aceptan hijos**: los accesorios de la
derecha (interruptor, botones, insignias, radio, casilla…) se eligen con propiedades. `CustomRow` acepta hijos solo
para casos únicos. Respetar esto mantiene chico el código generado y hace que la compilación de release no se quede
sin memoria.

## Seguridad, en corto

- Procesos solo por ruta absoluta, sin ventana y con argumentos separados; PowerShell solo con scripts fijos.
- Enlaces solo `ms-settings:`, `https:`, `windowsdefender:` y `ms-windows-store:`.
- El diario de ajustes se valida: una entrada solo puede restaurar claves de su propio ajuste.
- winget y los desinstaladores de `HKCU` no se ejecutan desde una sesión elevada (sus rutas las puede modificar
  cualquier programa del usuario).
- Ítems de inicio quitados: la copia de los de «todos los usuarios» vive en `HKLM\SOFTWARE\LiquidControl\StartupRemoved`
  (solo administradores pueden escribir ahí); restaurarlos nunca usa datos de la carpeta de datos.
- Red: solo HTTPS, hosts del catálogo, tamaño máximo y tiempo de espera; los íconos quedan en caché para usarse sin
  conexión. Única excepción: AirDrop, que por diseño habla HTTP plano **solo en la red local** (así funciona el
  protocolo de LocalSend) y viene apagado por defecto; ver `airdrop.rs` para sus propias protecciones (nombres de
  archivo saneados, validación de sesión/token, aceptación explícita del usuario).
