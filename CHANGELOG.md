# Changelog

Todos los cambios importantes de LiquidControl. El formato sigue [Keep a Changelog](https://keepachangelog.com/es-ES/1.1.0/)
y las versiones, [SemVer](https://semver.org/lang/es/).

## [0.5.3] - 2026-10-11

### Agregado

- **61 ajustes nuevos** en el catálogo, tomados del inventario de WinScript y ubicados en la sección que les
  corresponde, cada uno con su activación, su reversión y el valor anterior guardado:
  - *Privacidad*: más canales de telemetría y de informes de errores, servicios y tareas de diagnóstico,
    contenido promocionado y publicidad por política, Spotlight, sincronización de ajustes con la nube, permisos
    de apps (archivos, radios, capturas, IA, teléfono, dispositivos de confianza), voz en la nube, búsqueda sin
    web ni ubicación, Cortana, Copilot completo y telemetría de Office, Visual Studio, Media Player, CCleaner y NVIDIA.
  - *Escritorio*: centro de notificaciones, Widgets por política, «Inicio» del Explorador, pantalla azul
    detallada, cifrado automático de BitLocker, WPBT y, en «Inicio de sesión», cámara de la pantalla de bloqueo,
    mensajes detallados, Num Lock y biometría.
  - *Optimización*: inicio rápido, Storage Sense, CPU de Defender, IPv4 sobre IPv6, aislamiento del núcleo,
    políticas extra de Edge y de Brave.
  - *Juegos*: MPO, optimizaciones de pantalla completa y de juegos en ventana, Xbox Game Bar.
  - *Actualizaciones*: posponer 1 año, conexión medida, drivers, política de actualizaciones automáticas,
    servicios de Windows Update y actualizadores de Adobe y Google.
- 28 apps más en la lista de apps preinstaladas que se pueden quitar (Whiteboard, Skype, 3D Builder, Bing
  Finanzas, Netflix, etc.).

## [0.5.2] - 2026-10-11

### Cambiado

- El color de acento **Vidrio líquido** ahora es un **celeste translúcido**: un reflejo blanco arriba y el
  celeste que se ve a través del botón, en vez de un cian intenso y sólido. Texto, interruptores, controles
  deslizantes, la barra lateral y la muestra del selector usan el mismo vidrio; en tema claro el texto de los
  botones pasa a un azul oscuro para que se lea.

### Arreglado

- Una **línea blanca pegada al borde de arriba de la ventana** (la fila que Windows pinta para dibujar la
  sombra de una ventana sin marco): ahora se saca el borde de DWM y la forma de la ventana arranca debajo de esa
  fila.

### Agregado

- Escritorio → Menú Inicio: **Desactivar el menú Inicio de Windows**. En vez de un nombre de carpeta fijo,
  busca en `SystemApps` de cada PC la carpeta que trae `StartMenuExperienceHost.exe`, cierra el programa y la
  renombra con `.old` (tomando antes su propiedad y devolviéndosela a TrustedInstaller al revertir). Pide
  administrador, es de riesgo avanzado y se revierte desde la misma app. Nueva operación `systemApp` en el
  catálogo de ajustes, que identifica la app por su ejecutable.

## [0.5.0] - 2026-09-29

### Cambiado

- El proyecto completo pasa a **GPL-3.0-only**, compatible con la edición open source de Slint.
- README renovado y avisos de terceros separados en `THIRD_PARTY_NOTICES.md`.
- La release ahora incluye un ZIP portable, licencias y suma SHA-256.

### Eliminado

- Se quitó por completo el Asistente local y sus dependencias de inferencia (`candle` y `tokenizers`).
- Se retiraron las referencias a WinScript: el motor de ajustes es una implementación propia.

## [0.4.0] - 2026-09-17

### Agregado

- **Buscador de la barra lateral, muchísimo más entendedor**: ahora indexa también cada ajuste del catálogo
  (Privacidad, Escritorio, Juegos, Optimización, Accesibilidad, Windows Update), así que buscar "copilot" o
  "widgets" lleva directo a su interruptor. Entiende preguntas enteras ("¿dónde cambio el fondo de pantalla?"),
  sinónimos e inglés ("dark mode", "wallpaper") y tolera errores de tipeo ("blutooth").
- **Búsqueda de fondos de pantalla mejorada**: ahora junta Wallhaven con **Openverse** (fotos con licencia
  Creative Commons) y, como los dos sitios son en inglés, traduce con un diccionario propio las palabras en
  español que reconoce y manda las dos búsquedas en paralelo. Antes "montaña" daba 2 resultados casi al azar;
  ahora da los mismos ~30 que "mountain".
- **Buscar fotos de iCloud** por nombre, por fecha ("enero 2024") o por cómo se ven: color dominante ("azules",
  "verdes"), si son claras u oscuras, blanco y negro, y su forma (verticales, horizontales). No usa ningún modelo
  de IA: mira la miniatura que Windows ya tiene guardada. Respeta el permiso de descargar miniaturas de fotos que
  están solo en la nube.
- **AirDrop** (panel nuevo, debajo de Wi‑Fi): mandar y recibir archivos con otros equipos de la red local, sin nube
  y sin instalar nada aparte. Habla el protocolo abierto de **LocalSend v2**, así que también anda con celulares
  que tengan LocalSend. Implementación propia en Rust (`crates/core/src/airdrop.rs`), sin dependencias nuevas.
  Viene **apagado**: mientras esté apagado no se anuncia, no escucha y nadie lo ve. Nada se guarda en el disco sin
  que el usuario acepte el envío, y cada archivo se valida contra la sesión aceptada (equipo, token y tamaño) y se
  guarda con el nombre saneado dentro de `Descargas\LiquidControl`.
- AirDrop: **iPhone sin instalar nada**, en su propia subpágina («iPhone y celulares») con el QR grande, los pasos, lo compartido, cómo se protege y otras opciones. El código QR abre una página en Safari para mandar fotos y archivos a la PC
  (con la misma aceptación que cualquier envío) o bajar los que la PC comparta. La página y las descargas piden una
  clave al azar que cambia cada vez que se prende AirDrop.
- AirDrop: **vista previa de lo recibido** (como Fotos de iCloud): miniaturas de las fotos, ícono para el resto y
  aviso del códec HEIF si llegan fotos del iPhone. Las imágenes se abren con un clic; cualquier otro archivo solo se
  muestra en su carpeta, para que nada recibido se ejecute sin querer.
- AirDrop: **pausar, reanudar y cancelar** lo que se está recibiendo o enviando, con velocidad y tiempo restante.
  La pausa no corta la conexión (TCP frena al otro equipo solo) y cancelar borra lo que no llegó completo.
- AirDrop más rápido: varios archivos se mandan **en paralelo**, se copian en pedazos de 1 MB, el archivo se reserva
  entero en el disco y cada conexión se atiende al instante (antes esperaba hasta 80 ms). Con 100 archivos, de
  129 MB/s a ~1000 MB/s medido por loopback (`examples/bench_airdrop`); un archivo solo, ~550 MB/s. En la práctica el
  límite pasa a ser la red: Ethernet de 1 Gbps ≈ 110 MB/s.
- iCloud: la sección de Fotos tiene **buscador** y un botón **Ver todas** con una página propia (hasta 200 fotos,
  las más nuevas) que se puede recorrer con scroll. Pasar el mouse por una foto la agranda con un efecto de carta
  que se inclina según de qué lado está el cursor; un clic la abre con el visor de imágenes (no la carpeta) y el
  clic derecho ofrece **Descargar** (fuerza a iCloud a bajar una que esté solo en la nube) y **Eliminar** (a la
  Papelera).
- Ventana según la pantalla: el tamaño de diseño (1140×760) se achica hasta el mínimo si no entra en el monitor
  actual a la escala de Windows y al "Tamaño de la interfaz" elegido, y queda centrada. Antes era un tamaño fijo
  que podía no entrar en notebooks chicas o quedar recortado con el tamaño de interfaz al máximo.
- Fondo de pantalla: **12 colores sólidos** (antes 8), repartidos para llenar todo el ancho de la fila.
- AirDrop: el enlace a "iPhone y celulares sin app" pasa a ser un pie de página chico y discreto, en vez de una
  fila grande, para no competir en tamaño con el resto del panel.
- Pantallas: **Luz nocturna** con interruptor e intensidad propios, sin abrir Configuración de Windows.
- General: **Windows Update** (buscar actualizaciones y opciones para posponerlas).
- Apariencia: el fondo de pantalla vuelve a ser una sección, con **búsqueda de fondos gratis** de la comunidad.
- Apariencia: nuevo color de acento **Vidrio líquido**. En vez de un color plano, los botones, interruptores,
  controles deslizantes y menús se ven y se sienten como vidrio teñido (reflejo arriba, sombra al pie) en sus
  estados de hover y presionado.
- Mouse: **Menús y vistas previas instantáneos** (antes solo en Optimización) pasa a tener su propio interruptor
  acá, con su propia explicación.
- El nombre del proyecto pasa de "Ajustes" a **LiquidControl** en toda la app, la carpeta de datos
  (`%APPDATA%\LiquidControl`), la carpeta de descargas de AirDrop y el resto de los lugares donde aparecía.
- Escritorio: **sin apps agregadas recientemente** en Inicio, y **reloj interno en UTC** (para dual boot con
  Linux o macOS) en una nueva sección "Sistema".

### Arreglado

- AirDrop: las fotos mandadas desde **LocalSend en el iPhone llegaban vacías (0 bytes)**. LocalSend sube los
  archivos "en pedazos" (`Transfer-Encoding: chunked`) y el servidor solo entendía `Content-Length`. Ahora entiende
  los dos, y un archivo que no llega completo se descarta con error en vez de guardarse cortado.
- AirDrop: después de un envío fallido la sesión quedaba trabada y todo lo siguiente respondía "ocupado".
- AirDrop: apagarlo y prenderlo enseguida daba "puerto en uso".
- El contenido de cada panel queda centrado y ya no se corta al angostar la ventana.
- Teclado: la lista de idiomas y distribuciones se leía mal («invalid type: sequence»).
- El pie de texto de una sección se cortaba en la primera línea cuando era largo (se veía en AirDrop, Fondo de
  pantalla e iCloud): el texto estaba en un `HorizontalLayout`, donde Slint calcula mal el alto de un texto con
  salto de línea.
- La barra de progreso de una transferencia de AirDrop se llenaba desde el centro en vez de desde la izquierda.

## [0.3.0] - 2026-09-12

### Agregado

- **Consola de actividad** (Ajustes de la app → Consola de actividad): cada acción en vivo (registro, procesos,
  servicios, tareas, red y errores) con filtro, copiar y archivo `data\logs\ajustes.log`. Ninguna acción abre una
  ventana de consola aparte.
- Botón **Detalles** en cada ajuste del catálogo con las operaciones exactas que aplica.
- Sonido: **volumen y silencio por app**, con el ícono de cada app, actualizado en vivo.
- General → Ítems de inicio: **quitar con un clic** (con copia para restaurarlo) e íconos de cada programa.
- Energía: **crear, renombrar y eliminar planes** y restaurar los de fábrica desde la app.
- Pantallas: interruptor de **HDR** nativo.
- Apariencia: el **acento de Windows se elige en la app** (sin abrir Configuración) y opción de acento automático;
  el fondo de pantalla pasa a ser una subpágina de Apariencia con **Deshacer** y «Volver al fondo anterior».
- Aplicaciones: **íconos reales** de las apps instaladas y del catálogo (el ícono público del sitio oficial de cada
  app, descargado una sola vez y guardado para usarse sin conexión; se puede desactivar).
- Catálogo: **Raycast** (Microsoft Store) junto a Flow Launcher.
- iCloud dentro de Cuenta; miniaturas de fotos que están solo en la nube (con permiso) y aviso del códec HEIF.
- Ajustes de la app: **tamaño de la interfaz** (90–130 %), intensidad de las **animaciones** (ninguna a máximas) y
  del **Liquid Glass** (0–150 %).
- Más Liquid Glass y animaciones: transición entre páginas, rebote al presionar, diálogos y avisos animados.
- Accesibilidad: interruptor de la Lupa.
- `docs/ARQUITECTURA.md`: qué archivo controla cada panel y cómo ver y modificar cada acción.
- Aplicaciones: **desinstalación profunda** al estilo Revo Uninstaller: después del desinstalador, LiquidControl busca
  carpetas, accesos directos y claves del registro que quedaron y deja elegir qué borrar (los archivos van a la
  Papelera y cada clave se respalda en un `.reg`).
- **Juegos** como sección propia de la barra lateral.
- Sonido: elegir la **salida y la entrada de audio** con un clic.
- Teclado: **idiomas y distribuciones** desde la app (teclado predeterminado, agregar y quitar).
- Íconos grises por opción en Privacidad, Escritorio y Juegos; el detalle de cada ajuste se desplaza si es largo y
  tiene botón **Copiar**.
- Energía: «Máximo rendimiento» aparece atenuado en la lista de planes y se agrega y activa con un toque.
- Ajustes de la app: **esquinas de la ventana** nativas de Windows o más redondeadas, al estilo macOS.

### Cambiado

- Botones al estilo de Ajustes del Sistema: solo texto, sin fondo, con una cápsula que aparece al presionarlos;
  más separados entre sí.
- Las listas se actualizan fila por fila en lugar de rearmarse: los paneles responden más rápido y un clic ya no se
  pierde si llega justo cuando se recargan los datos.
- Las filas de la interfaz ya no aceptan hijos (`@children`): el código que genera Slint es mucho más chico y la
  compilación de release ya no se queda sin memoria.
- Registro de paneles en `src/panes/mod.rs`: cada panel declara su `PANE` (iniciar, entrar, salir, subpáginas).
- Seguridad: el diario de ajustes se valida (una entrada solo puede tocar claves de su ajuste), winget y los
  desinstaladores de `HKCU` no se ejecutan como administrador, la red usa solo HTTPS con tamaño máximo, y la copia
  de los ítems de inicio «para todos los usuarios» se guarda en `HKLM` (solo administradores) en lugar de la carpeta
  de datos, para que ningún programa pueda cambiar lo que se restaura con permisos elevados.

### Quitado

- «Reducir movimiento» de Ajustes de la app (lo reemplaza la intensidad de las animaciones).
- Fondo de pantalla e iCloud como paneles sueltos de la barra lateral.

## [0.2.0] - 2026-09-12

### Cambiado

- Interfaz reescrita en **Rust nativo con Slint** (Skia por GPU con respaldo por software): sin WebView2, pensada
  para usar menos de 100 MB de RAM (la 0.1.0 usaba ~345 MB).
- Proyecto organizado como workspace de Cargo: `ajustes-core` (todo el acceso al sistema) y `ajustes` (interfaz).
- Todo el backend es real: se quitó el backend simulado que usaba la versión web.

### Agregado

- Ventana sin marco con la sombra y las esquinas redondeadas de Windows 11; semáforo de macOS o botones de Windows.
- Barra lateral flotante con búsqueda que ignora tildes y guiones, historial Atrás/Adelante, avisos y diálogos.
- Los 20 paneles portados con datos reales del sistema.
- Selector de archivos nativo para el fondo de pantalla.
- Ícono, datos de versión y manifiesto (DPI por monitor, UTF-8, rutas largas) dentro del `.exe`.
- Integración continua en GitHub Actions (formato, lint, pruebas y ejecutable en cada versión).

### Quitado

- Tauri, WebView2, Svelte, Vite y la dependencia de Node.js.

## [0.1.0] - 2026-09-12

- Primera versión: Tauri 2 + Svelte 5 con Liquid Glass por filtros SVG.
