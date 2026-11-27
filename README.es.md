<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.zh-CN.md">简体中文</a> ·
  <a href="README.zh-TW.md">繁體中文</a> ·
  <a href="README.ja.md">日本語</a> ·
  <a href="README.de.md">Deutsch</a> ·
  <b>Español</b> ·
  <a href="README.pt-BR.md">Português (Brasil)</a> ·
  <a href="README.ru.md">Русский</a> ·
  <a href="README.tr.md">Türkçe</a>
</p>

<p align="center"><sub>Esta es una traducción; la referencia es el <a href="README.md">README</a> en inglés.</sub></p>

<img alt="rustyfiles: un gestor de archivos para Windows y macOS que nunca se congela. Escrito en Rust. Dibujado como una ventana de rustyfiles sobre una carpeta de 100,000 archivos, abierta en 281 ms" src="docs/readme/cover.png" width="100%">

<p align="center">
  <b>Un gestor de archivos que nunca espera al disco.</b><br>
  Pestañas, doble panel, archivos que se abren como carpetas y una vista previa con <kbd>Space</kbd>.<br>
  Escrito desde cero en Rust, para Windows y ahora también para macOS.
</p>

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest"><img alt="Última versión" src="https://img.shields.io/github/v/release/gelenmedya/rustyfiles?label=download&labelColor=232326&color=ff6a1a"></a>
  <a href="https://github.com/gelenmedya/rustyfiles/releases"><img alt="Descargas" src="https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fgelenmedya%2Frustyfiles%2Fbadges%2Fdownloads.json"></a>
  <img alt="Windows 11, x64 y ARM64" src="https://img.shields.io/badge/Windows_11-x64_%7C_ARM64-3d3d44?labelColor=232326">
  <img alt="macOS 26 en Apple silicon e Intel, beta" src="https://img.shields.io/badge/macOS_26-Apple_silicon_and_Intel_(beta)-3d3d44?labelColor=232326">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest"><img alt="Cada versión se analiza en VirusTotal: los informes están enlazados al final de sus notas" src="https://img.shields.io/badge/VirusTotal-every_release_scanned-3d3d44?labelColor=232326"></a>
  <a href="LICENSE"><img alt="Licencia MIT" src="https://img.shields.io/badge/license-MIT-3d3d44?labelColor=232326"></a>
</p>

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe"><img alt="Descargar para Windows" src="https://img.shields.io/badge/Download_for_Windows-FF6A1A?style=for-the-badge&logo=windows11&logoColor=white"></a>
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-mac.dmg"><img alt="Descargar para macOS (beta)" src="https://img.shields.io/badge/Download_for_macOS_(beta)-232326?style=for-the-badge&logo=apple&logoColor=white"></a>
  <br>
  <a href="https://rustyfiles.com"><img alt="Sitio web" src="https://img.shields.io/badge/rustyfiles.com-232326?style=flat-square&logo=googlechrome&logoColor=white"></a>
  <a href="https://discord.gg/K2banZ36zJ"><img alt="Únete al Discord" src="https://img.shields.io/badge/Discord-5865F2?style=flat-square&logo=discord&logoColor=white"></a>
  <a href="https://www.buymeacoffee.com/ozerguzel"><img alt="Invítame a un café" src="https://img.shields.io/badge/Buy_me_a_coffee-FFDD00?style=flat-square&logo=buymeacoffee&logoColor=black"></a>
</p>

<table>
  <tr>
    <td width="33%" valign="top">
      <b>Nunca espera al disco</b><br>
      El listado, los iconos, las miniaturas, el menú contextual y cada copia se ejecutan fuera del hilo de la ventana.
      Un recurso compartido de red lento o una carpeta de 100,000 archivos no pueden congelarla.
    </td>
    <td width="33%" valign="top">
      <b>El trabajo del Explorador, hecho más rápido</b><br>
      Pestañas, doble panel, archivos comprimidos como carpetas, un índice de nombres para búsquedas instantáneas, renombrado masivo, deshacer,
      y su propio motor de copia: 14 veces más rápido con archivos pequeños.
    </td>
    <td width="33%" valign="top">
      <b>Gratuito y tuyo</b><br>
      Sin cuenta, sin anuncios, sin enviar nada a ningún sitio. Licencia MIT, y cada línea está aquí para leerla.
      Las actualizaciones se instalan en silencio en segundo plano.
    </td>
  </tr>
</table>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/readme/projects-dark.png">
  <img alt="rustyfiles mostrando una carpeta Proyectos en vista de detalles, con pestañas, una barra de ruta y carpetas fijadas en la barra lateral; la ventana es de cristal sobre el fondo de pantalla del escritorio" src="docs/readme/projects-light.png" width="100%">
</picture>

<sub>Windows, con el fondo experimental HyperGlass (Configuración › Apariencia). El predeterminado es Mica, como en el Explorador.</sub>

## Velocidad

<img alt="Una prueba de velocidad dibujada como una lista de rustyfiles: abrir una carpeta de 100,000 archivos tarda 281 ms en rustyfiles y 2,318 ms en el Explorador de archivos (8 veces más rápido); una carpeta de 2,000 imágenes, 278 ms frente a 2,177 ms (8 veces); copiar 10,000 archivos pequeños, 1.8 s frente a 25.6 s (14 veces); extraer un ZIP de 10,000 archivos, 3.6 s frente a 32.4 s (9 veces)" src="docs/readme/speed.png" width="100%">

Una carpeta de 100,000 archivos se abre en 281 ms (Explorador de archivos: 2.3 s). Las cifras provienen de la prueba de velocidad
integrada en la app, Configuración › Prueba de velocidad, para que puedas comprobarlas en tu propio PC.

<details>
<summary><b>Cada medición y cómo se tomó</b></summary>

#### Abrir una carpeta

Tiempo desde que se inicia la app hasta que la lista de la carpeta deja de moverse, en una ventana nueva, con la carpeta ya en la caché de archivos del sistema. Mediana de tres ejecuciones tras un calentamiento.

| Carpeta | rustyfiles | Explorador de archivos | [Files](https://files.community) |
|---|--:|--:|--:|
| Carpeta vacía | 281 ms | 1,006 ms | 813 ms |
| 1,000 archivos | 284 ms | 2,214 ms | 1,893 ms |
| 10,000 archivos | 286 ms | 2,221 ms | 1,053 ms |
| 100,000 archivos | 281 ms | 2,318 ms | 4,069 ms |
| 2,000 imágenes (miniaturas) | 278 ms | 2,177 ms | 1,075 ms |
| Memoria tras listar 10,000 archivos | 39 MB | 69 MB | 204 MB |

Medido en un equipo (Ryzen 7 9800X3D, SSD NVMe, Windows 11) el 2026-11-16, con rustyfiles
0.8.6 y Files 4.3. rustyfiles y el Explorador iniciaron cada uno un proceso nuevo. Files ya estaba
en ejecución en segundo plano, así que sus tiempos no son los de un inicio en frío; además, rellena su lista
por pasos, y el tiempo cuenta hasta que la imagen deja de cambiar. La memoria es el conjunto de trabajo privado, la cifra
que muestra el Administrador de tareas. Con «seguir ejecutándose en segundo plano» activado, una ventana nueva de rustyfiles no inicia
ningún proceso y mostró una carpeta listada en 93–168 ms (2026-11-06, una versión anterior).

#### Una carpeta de 100,000 archivos que no deja de cambiar

La carpeta abierta, un archivo seleccionado y unos 1,000 archivos renombrados por segundo en segundo plano
durante 5 segundos:

| | |
|---|--:|
| Desde el inicio hasta una carpeta listada (100,001 archivos) | 315 ms |
| La lista sigue los cambios | cada ~750 ms |
| Releer la carpeta (hilo en segundo plano) | ~70 ms |
| Trabajo en el hilo de la interfaz por actualización | 7–8 ms |
| Selección y cursor | se mantienen en el archivo seleccionado |

Tiempos de fotograma mientras esa misma carpeta se desplaza hacia abajo y de vuelta arriba durante 7.7 s, con la selección
(y por tanto el panel de vista previa) moviéndose cada medio segundo, unos 680 archivos renombrados por segundo en segundo plano,
en una pantalla de 200 Hz:

| | mediana | p95 | p99 | peor |
|---|--:|--:|--:|--:|
| Coste por fotograma en el hilo de la interfaz | 2.1 ms | 4.6 ms | 7.9 ms | 16.8 ms |
| Tiempo entre fotogramas | 3.3 ms | 5.3 ms | 9.9 ms | 17.3 ms |

2,182 fotogramas en 7.7 s; un hueco más largo que un fotograma a 60 Hz. Mídelo tú mismo con
`RUSTYFILES_TRACE_LIVE=1` y `RUST_LOG=warn,rf_app=info`, y registra los fotogramas con
`GPUI_FRAME_LOG=frames.tsv`.

#### Nueva pestaña en una ventana abierta

| Carpeta | Tiempo hasta una lista estable |
|---|--:|
| 1,000 archivos | ~48 ms |
| 10,000 archivos | ~46 ms |
| 100,000 archivos | ~129 ms |

Cada pestaña adicional cuesta unos 3 MB de memoria (1 pestaña: ~32 MB, 25 pestañas: ~103 MB).

#### Buscar por nombre de archivo

rustyfiles mantiene un índice de nombres de las unidades locales y responde a las búsquedas desde él. Búsqueda de
«rapor» en todas las subcarpetas de una carpeta de usuario con 3.6 millones de archivos y carpetas, medida hasta que
los resultados en pantalla dejaron de cambiar:

| | Tiempo |
|---|--:|
| rustyfiles 0.4 | **0.38 s** |
| Explorador de archivos | 1.8 s (su más rápido de 4 ejecuciones; las demás tardaron 11–32 s) |

El índice guarda cada nombre distinto una sola vez: una unidad con 4.8 millones de entradas pero 490 mil nombres distintos
ocupa 61 MB. Un vigilante de carpetas lo mantiene al día mientras rustyfiles está abierto; entre ejecuciones, el diario de cambios
de NTFS (leído sin derechos de administrador) indica qué carpetas cambiaron, así que un inicio solo las vuelve a listar: 0.2 s para C: aquí,
donde un recorrido tarda 30 s. Quedan fuera los datos de aplicaciones, las cachés, `node_modules` y `.git`; una búsqueda
iniciada dentro de uno de ellos recorre el disco:

| `*.dll` en `C:\Windows` (342,492 elementos, 116,851 carpetas, con caché cargada) | Tiempo |
|---|--:|
| rustyfiles | **7.8 s** |
| `dir /s /b` | 53 s |
| PowerShell `Get-ChildItem -Recurse` | 105 s |

Busca en seis hilos de prioridad baja, así que el resto del equipo sigue respondiendo, y no sigue los puntos de unión
(junctions), por eso encuentra 454 duplicados menos.

#### Vista previa con Space (DirectLook)

| Archivo | Contenido listo, desde la pulsación de tecla |
|---|--:|
| JPEG, 3840 × 2160 | 24–86 ms |
| Código fuente, con resaltado de sintaxis | 23–44 ms |
| Archivo 7z (contenido listado) | 19–31 ms |
| MP3 (etiquetas, letras) | 27–32 ms |
| Vídeo H.264, 1080p | primer fotograma en 146–185 ms |

#### Archivos comprimidos

Lectura de RAR con [rarchive](https://github.com/gelenmedya/rarchive), nuestro propio motor, frente al UnRAR oficial
(decodificar y comprobar cada CRC, archivos en la caché del sistema):

| Archivo | UnRAR | rarchive |
|---|--:|--:|
| 1.3 GB, diccionario de 32 MB, 195 archivos | 2.79 s | **0.28 s** (10×) |
| 400 MB sólido | 0.95 s | **0.21 s** (4.5×) |
| 400 MB, 520 archivos | 1.03 s | **0.063 s** (16×) |

#### Copiar y extraer

Nuestros motores frente a los de Windows (rustyfiles 0.8.6, 2026-11-16):

| | Windows | rustyfiles |
|---|--:|--:|
| Copiar 10,000 archivos pequeños en 100 carpetas (41 MB) | 25.6 s | **1.8 s** (14×) |
| Copiar un archivo de 1 GB | 0.61 s | 0.44 s |
| Extraer un ZIP de 10,000 archivos | 32.4 s | **3.6 s** (9×) |

Copiar reparte los archivos pequeños entre varias carpetas, porque los hilos que escriben en una misma carpeta se esperan
unos a otros; un archivo grande está limitado por el disco, no por el motor. Gestiona los conflictos de nombre por sí mismo
(reemplazar, omitir o conservar ambos), pausa y reanuda, puede verificar cada copia byte a byte y puede limitar su velocidad.
El script de prueba de copia es `crates/rf-win/examples/copybench.rs`.

Todas las cifras: un equipo (Ryzen 7 9800X3D, SSD NVMe, Windows 11), noviembre de 2026.

</details>

## Qué hace

- **Navegación.** Pestañas que puedes arrastrar fuera a una ventana nueva, sesiones guardadas, doble panel, vistas de detalles, cuadrícula, lista y columnas, agrupación, panel de vista previa, etiquetas de color, el Shelf, recursos compartidos de red y FTP.
- **Archivos.** Deshacer y rehacer, renombrado masivo, tamaños de carpeta bajo demanda, uso de disco con un mapa de árbol, el menú contextual completo del Explorador, y arrastrar y soltar con cualquier otra app.
- **Archivos comprimidos.** ZIP, 7z, RAR, tar e ISO se abren como carpetas; ZIP, 7z y tar se pueden editar por dentro.
- **Búsqueda.** Un índice de nombres de tus unidades, y búsqueda rápida (<kbd>Alt</kbd>+<kbd>Space</kbd>) para apps, archivos, marcadores y las configuraciones de Windows.
- **Windows.** Opcionalmente, el gestor de archivos predeterminado, incluidos <kbd>Win</kbd>+<kbd>E</kbd> y los cuadros Abrir y Guardar de otras apps.
- **49 idiomas**, siguiendo el del sistema.

<details>
<summary><b>Todas las funciones en detalle</b></summary>

**Navegación.** Pestañas que puedes arrastrar para reordenarlas o sacarlas a una ventana nueva. Guarda las pestañas de una ventana,
incluidos los paneles divididos, como una sesión con nombre y ábrela de nuevo más tarde. Una pestaña nueva abre Inicio o la carpeta
en la que estás (Configuración › General). Doble panel. Vistas de detalles, cuadrícula, lista y columnas (la lista puede llenarse
columna a columna, como en el Explorador), elementos compactos como la vista compacta del Explorador, agrupación, panel de vista previa,
búsqueda, etiquetas de color, el Shelf, bibliotecas, recursos compartidos de red y FTP. Una columna de estado de Git.
Tamaños de carpeta bajo demanda: haz clic en «Calcular tamaño»; se recuerdan al reiniciar y pueden mostrar el tamaño en disco.
«Mostrar uso de disco» en una carpeta o unidad enumera lo que ocupa más espacio, del mayor al menor, junto a un mapa de árbol
en el que puedes hacer clic para navegar. Las carpetas virtuales como Bibliotecas y Galería se abren en rustyfiles, y los
dispositivos que Windows lista en Este equipo (teléfonos por USB o Phone Link) aparecen en Dispositivos en la página de Inicio.
Las carpetas en la nube que Windows conoce (OneDrive, Proton Drive, Dropbox, MEGA y otras) están en la barra lateral.

**Archivos.** Deshacer y rehacer para renombrar, copiar, mover, eliminar a la Papelera de reciclaje y aplanar carpetas.
Renombrado masivo. Duplicar, copiar o mover al otro panel, y pegar texto copiado o una imagen como archivo nuevo. El menú
contextual completo del Explorador con Enviar a, arrastrar y soltar con el Explorador, el escritorio, los navegadores y Outlook,
y destinos para soltar en pestañas, carpetas fijadas, carpetas y programas. Envía archivos a tus otros equipos con [Ferry](#ferry)
y descarga con [HyperDownload](#hyperdownload).

**Archivos comprimidos.** ZIP, 7z, RAR, la familia tar e ISO se abren como carpetas, incluidos los ZIP, 7z y RAR protegidos
con contraseña. Añadir, eliminar y renombrar dentro de ZIP, 7z y tar. Crear ZIP, 7z y tar.zst en todos los núcleos. La lectura
la hace [rarchive](https://github.com/gelenmedya/rarchive), nuestro propio motor en Rust puro (también
[en crates.io](https://crates.io/crates/rarchive)): descomprime un archivo WinRAR de 1.3 GB diez veces más rápido que UnRAR.

**Windows.** Opcionalmente, el gestor de archivos predeterminado: las carpetas, <kbd>Win</kbd>+<kbd>E</kbd>, Este equipo y la
Papelera de reciclaje se abren en rustyfiles. Opcionalmente, los cuadros Abrir, Guardar y Seleccionar carpeta de otras apps
también se abren en rustyfiles (Configuración › Avanzado), y un botón de fijar allí guarda una carpeta de inicio para cada app.
Las imágenes y vídeos abiertos en Fotos u otra app de la Tienda llegan con su carpeta, así que sus botones Anterior y Siguiente
funcionan. Las rutas `/select,` y `shell:` del Explorador funcionan. Mica, el modo oscuro y el color de énfasis del sistema.
Cuando un juego o un render satura la GPU, el dibujo pasa a la CPU y vuelve sin parpadeos.

**Comandos en la barra de direcciones.** Igual que en el Explorador: pulsa <kbd>Ctrl</kbd>+<kbd>L</kbd>, escribe `cmd`,
`powershell`, `wt` o `code .` y pulsa <kbd>Enter</kbd>. El texto se interpreta primero como un lugar (una ruta, una unidad,
`%VAR%`, `shell:`); una carpeta o archivo con ese nombre en la carpeta abierta siempre tiene prioridad. Solo cuando no existe
tal lugar, la primera palabra se busca como programa, igual que en el cuadro Ejecutar, y se inicia en la carpeta abierta.
No se inicia nada desde una autocompletación, y no se piden derechos de administrador. Un interruptor en Configuración › Archivos
y carpetas › Comportamiento lo desactiva.

**Organización.** Reglas para una carpeta: condiciones sobre el nombre, la extensión, el tipo, el tamaño y la antigüedad, y una
acción (mover, mover a una carpeta creada a partir de una plantilla, renombrar o la Papelera de reciclaje). Una ejecución manual
muestra cada paso en una vista previa y no mueve nada antes de Aplicar, y una ejecución se puede deshacer. No se elimina nada
de forma definitiva y no se sobrescribe nada. Las reglas que se ejecutan solas están desactivadas por defecto.

**Búsqueda.** La búsqueda rápida (<kbd>Alt</kbd>+<kbd>Space</kbd>) encuentra apps, archivos y carpetas, marcadores del navegador
y las páginas de configuración de Windows. Lo que abres con frecuencia aparece primero, y lo que elegiste antes para las mismas
letras es la primera fila.

**Idiomas.** Son 49, siguiendo el idioma de visualización del sistema (o elige uno en Configuración › General). La interfaz
está escrita en turco e inglés; los demás idiomas partieron de la redacción de la comunidad de Files y se traducen
a máquina a partir de ahí. Los nombres y textos de derecha a izquierda se leen de derecha a izquierda (gracias a @ialhabbal);
la interfaz en sí todavía no se refleja. Las correcciones son bienvenidas: cada idioma es un archivo JSON en
`crates/rf-app/locales/`.

</details>

<img alt="rustyfiles mostrando una carpeta con los fondos de escritorio de Windows en vista de cuadrícula, uno de ellos seleccionado y mostrado en el panel de vista previa de la derecha, sobre el fondo HyperGlass" src="docs/readme/grid.png" width="100%">

## DirectLook

Selecciona un archivo y pulsa <kbd>Space</kbd>: vídeo y audio con su propio reproductor, imágenes, texto y código, el contenido
de los archivos comprimidos, y una tarjeta de detalles para todo lo demás. Las teclas de flecha recorren la carpeta mientras
sigue abierta. Se ejecuta como programa propio, así que un vídeo pesado nunca carga el gestor de archivos, y si lo activas
también funciona en el Explorador y en el escritorio (Configuración › DirectLook).

## Ferry

Envía archivos y carpetas a rustyfiles en tus otros equipos de la misma red, igual que AirDrop:
clic derecho › **Enviar con Ferry**. Desactivado hasta que lo actives en Configuración › Ferry.

<img alt="Configuración › Ferry con dos dispositivos cercanos, un PC con Windows emparejado con su velocidad medida y un MacBook con una velocidad estimada; una notificación de Windows pregunta si aceptar archivos del MacBook, y la barra de estado muestra una transferencia al 61 por ciento" src="docs/readme/ferry.png" width="100%">

<sub>Un dibujo de Configuración › Ferry con dispositivos inventados, con las palabras y la disposición de la propia app.</sub>

<details>
<summary><b>Cómo funciona Ferry</b></summary>

- Cifrado de extremo a extremo (QUIC con TLS 1.3), directamente de dispositivo a dispositivo: sin servidor, sin cuenta,
  nada sale de tu red.
- Empareja tus dispositivos como emparejas Bluetooth: ambos muestran el mismo código de seis dígitos. Los archivos de un
  dispositivo emparejado pueden llegar sin preguntar; cualquier otra cosa se pregunta primero, en una notificación.
- Tú eliges quién puede ver tu equipo: nadie, tus dispositivos emparejados (la opción predeterminada), o todos durante
  diez minutos. Mientras Configuración › Ferry está abierta, todos pueden verlo, para que un dispositivo nuevo pueda encontrarlo.
- Una transferencia interrumpida continúa desde donde se detuvo, y cada archivo se comprueba antes de tomar su nombre.
  Los archivos recibidos se marcan como descargados, así que SmartScreen pregunta antes de que se ejecute un programa entre ellos.
- Cada dispositivo muestra una velocidad estimada, y **Probar velocidad** la mide en ambos sentidos en unos pocos segundos.
- La app de macOS habla el mismo protocolo, descrito en [docs/nearby-protocol.md](docs/nearby-protocol.md).
  El Firewall de Windows puede preguntar por Ferry: permítelo para redes privadas.

</details>

## HyperDownload

El gestor de descargas propio de rustyfiles, un complemento: Configuración › Complementos › HyperDownload › **Instalar**. Un archivo
por muchas conexiones a la vez, pausado y reanudado donde se detuvo, torrents y enlaces magnet, una página de Descargas, y
vídeos que se reproducen en DirectLook antes de terminar de descargarse. Es un programa aparte ([código fuente](https://github.com/gelenmedya/hyperdownload), MIT),
que se descarga solo cuando haces clic en Instalar, se comprueba primero con su SHA-256 y se puede quitar en cualquier momento.

## Experimental en Windows: HyperGlass, el Dock y el escritorio

Desactivado por defecto y en fase temprana: construido y probado en un equipo con dos monitores. Espera asperezas y, por favor,
cuéntanos lo que encuentres. El Explorador sigue ejecutándose por debajo; no se elimina ni se reemplaza nada de Windows en el disco.

<img alt="Un escritorio con el Dock de rustyfiles abajo (tres islas: Inicio y búsqueda, las apps fijadas y en ejecución, la bandeja y el reloj), el panel Inicio del Dock abierto a la izquierda con apps fijadas y archivos recientes, HyperSearch en el centro con las apps encontradas para adobe, y el centro de control a la derecha con Wi-Fi, Bluetooth, Ethernet, la canción que suena, volumen y brillo" src="docs/readme/shell.png" width="100%">

<sub>Inicio, HyperSearch y el centro de control, juntos a partir de tres capturas del mismo escritorio (no se abren todos a la vez).</sub>

<details>
<summary><b>Qué hace cada parte</b></summary>

- **HyperGlass** (Configuración › Apariencia) es un quinto fondo: el fondo de la ventana es cristal que dobla y esmerila
  lo que hay detrás.
- **El Dock** (Configuración › Dock, un solo interruptor) coloca tres islas de cristal en la parte inferior de cada monitor
  en lugar de la barra de tareas de Windows: un panel Inicio y búsqueda propios, las apps en ejecución y fijadas con vistas
  previas de ventanas, los iconos de la bandeja, un centro de control y un panel de notificaciones. Mientras está activo, pulsar
  la tecla de Windows abre el Inicio del Dock, y <kbd>Alt</kbd>+<kbd>Tab</kbd> abre su selector. Desactiva el Dock y vuelven los
  de Windows.
- **El escritorio** dibujado por rustyfiles llega con el Dock: tu fondo de pantalla e iconos, con una opción para las esquinas redondeadas.
- **Brillo para cada monitor.** Windows solo tiene un control deslizante para la pantalla propia de un portátil; el centro de control
  del Dock tiene uno para cada monitor. Los monitores externos se configuran por DDC/CI, que casi todos los monitores de escritorio
  de los últimos quince años hablan por DisplayPort, HDMI y DVI. Si un monitor no tiene control deslizante, comprueba que DDC/CI
  esté activado en su propio menú; algunas bases, conmutadores KVM y adaptadores no lo transmiten.

</details>

## macOS (beta)

El mismo gestor de archivos, dibujado de forma nativa con AppKit y Liquid Glass sobre el mismo núcleo en Rust: pestañas,
doble panel, todas las vistas, Quick Look con <kbd>Space</kbd>, archivos comprimidos como carpetas, renombrado masivo, uso de disco,
el Shelf, etiquetas de color, estado de Git, Conectar al servidor, Ferry y los mismos 49 idiomas. El Dock, el escritorio,
HyperSearch y DirectLook siguen en Windows por ahora, y la versión para Mac de HyperDownload llegará en su propia versión.
Apple silicon e Intel, con macOS 26 (Tahoe). Se actualiza sola, igual que la app de Windows.

## Instalación

**Windows.** [Descargar rustyfiles-win-Setup.exe](https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe)
(unos 50 MB) y ábrelo. Si Windows dice «Windows protegió su PC», haz clic en **Más información** y luego en
**Ejecutar de todas formas**: el instalador todavía no tiene firma de código, y cada versión se analiza en
[VirusTotal](https://www.virustotal.com) (los informes están enlazados al final de sus notas de versión).
rustyfiles se abre solo y aparece en tu menú Inicio. No hace falta contraseña de administrador; las actualizaciones
se instalan en silencio en segundo plano.

**macOS (beta).** [Descargar rustyfiles-mac.dmg](https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-mac.dmg),
ábrelo y arrastra rustyfiles a Aplicaciones. Apple lo ha firmado y notarizado, así que se abre como cualquier otra app de Mac.

<details>
<summary><b>Scoop, portátil, ARM64, gestor de archivos predeterminado, desinstalación</b></summary>

**Con [Scoop](https://scoop.sh):**

```powershell
scoop bucket add rustyfiles https://github.com/gelenmedya/scoop-rustyfiles
scoop install rustyfiles/rustyfiles
```

**Portátil o ARM64:** un zip portátil y un instalador ARM64 (`rustyfiles-win-arm64-Setup.exe`) están
en la página de [Versiones](https://github.com/gelenmedya/rustyfiles/releases). Se requiere Windows 11
(Windows 10 no está probado). Linux está previsto.

**Carpetas y <kbd>Win</kbd>+<kbd>E</kbd> en rustyfiles:** activa Configuración › Avanzado › Establecer como
gestor de archivos predeterminado. Puedes desactivarlo en cualquier momento.

**Desinstala** como cualquier app, desde Configuración de Windows › Aplicaciones. Todo lo que rustyfiles cambió vuelve al
Explorador de archivos.

**¿Fallan o se cierran los cuadros Abrir/Guardar de otras apps** después de que se eliminaran los archivos de rustyfiles sin
desinstalar (un antivirus, una actualización rota)? Volver a iniciar rustyfiles lo arregla. Si ya no se inicia, este comando de
PowerShell devuelve a todas las apps el cuadro de diálogo propio de Windows:

```powershell
Remove-Item -Recurse "HKCU:\Software\Classes\CLSID\{DC1C5A9C-E88A-4dde-A5A1-60F82A20AEF7}", "HKCU:\Software\Classes\CLSID\{C0B4E2F3-BA21-4773-8DBA-335EC946EB8B}"
```

</details>

<details>
<summary><b>Compilar desde el código fuente</b></summary>

Necesitas [Rust](https://rustup.rs) 1.95 o posterior y las Build Tools de Visual Studio con la
carga de trabajo «Desarrollo para el escritorio con C++» (una dependencia, zstd, compila código C).

```powershell
git clone https://github.com/gelenmedya/rustyfiles
cd rustyfiles
.\scripts\install.ps1            # builds, then installs for your user
.\scripts\install.ps1 -Default   # also makes it the default file manager
.\scripts\install.ps1 -Uninstall
```

`cargo run --release -p rf-app` la inicia sin instalarla. La app de macOS vive en `macos/` y
`crates/rf-mac`, fuera del espacio de trabajo de Windows.

</details>

**¿Preguntas o ideas?** Pásate a saludar en [Discord](https://discord.gg/K2banZ36zJ), o visita
[rustyfiles.com](https://rustyfiles.com).

## Por dentro

<details>
<summary><b>Teclado</b></summary>

| Teclas | Acción |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>T</kbd> / <kbd>Ctrl</kbd>+<kbd>W</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd> | Pestaña nueva, cerrar pestaña, reabrir pestaña cerrada |
| <kbd>Ctrl</kbd>+<kbd>L</kbd> o <kbd>Alt</kbd>+<kbd>D</kbd> | Barra de direcciones |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> | Buscar (<kbd>Enter</kbd> busca también en las subcarpetas) |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> o <kbd>F1</kbd> | Paleta de comandos |
| <kbd>Ctrl</kbd>+<kbd>P</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>A</kbd> | Buscar una carpeta visitada, buscar una pestaña |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>1</kbd> … <kbd>5</kbd> | Detalles, cuadrícula, columnas, tarjetas, lista |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd>, <kbd>F6</kbd> | Doble panel, cambiar de panel |
| <kbd>Shift</kbd>+<kbd>F5</kbd> / <kbd>Shift</kbd>+<kbd>F6</kbd> | Copiar, mover al otro panel |
| <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>D</kbd> | Duplicar |
| <kbd>Alt</kbd>+<kbd>P</kbd>, <kbd>Ctrl</kbd>+<kbd>B</kbd> | Panel de vista previa, barra lateral |
| <kbd>Space</kbd> | DirectLook |
| <kbd>F2</kbd> | Renombrar (renombrado masivo con varios elementos seleccionados) |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> / <kbd>Ctrl</kbd>+<kbd>Y</kbd> | Deshacer, rehacer |
| <kbd>Ctrl</kbd>+<kbd>H</kbd> | Mostrar elementos ocultos |

Cada atajo se puede cambiar en Configuración › Acciones: haz clic en un comando y luego pulsa las nuevas teclas.

</details>

<details>
<summary><b>Estructura del proyecto</b></summary>

| Crate | Qué contiene |
|---|---|
| `crates/rf-app` | La interfaz de Windows, y sus dos programas: `rustyfiles` y `directlook` (`src/look/`) |
| `crates/rf-win` | Integración con Windows y con el shell, el motor de operaciones de archivos |
| `crates/rf-core` | El modelo compartido por ambas plataformas: ubicaciones, historial, entradas, formatos |
| `crates/rf-archive` | Operaciones de archivos comprimidos: explorar, extraer, crear, editar |
| `crates/rf-nearby` | Ferry, compartido con la app de macOS |
| `crates/rf-player` | El reproductor de vídeo y audio de DirectLook |
| `crates/rustdecoder` | Nuestro propio motor de vídeo, en curso: MP4/MOV y MKV/WebM; H.264, HEVC, VP9 y AV1 decodificados en la tarjeta gráfica mediante D3D11 Video, con resultados idénticos bit a bit a ffmpeg / dav1d en sus pruebas; ProRes y DNxHR en la CPU; audio y sincronía A/V ([diseño](docs/design/rustdecoder.md)) |
| `macos/`, `crates/rf-mac` | La app de macOS (AppKit) y su puente con el núcleo en Rust |

La interfaz de Windows está construida con [GPUI](https://gpui.rs), el framework detrás del editor Zed. Su
capa de Windows es una bifurcación que añade el cambio GPU/CPU:
[gelenmedya/gpui-pre-windows](https://github.com/gelenmedya/gpui-pre-windows).

</details>

<details>
<summary><b>Variables de entorno</b></summary>

| Variable | Efecto |
|---|---|
| `RUSTYFILES_RENDERER` = `gpu`, `cpu` o `auto` | Forzar dónde se dibuja la ventana |
| `RUSTYFILES_CONFIG_DIR` = carpeta | Guardar la configuración en otra carpeta (uso portátil) |
| `RUSTYFILES_MULTI_INSTANCE=1` | Permitir varias copias a la vez (pruebas) |
| `RUSTYFILES_FRAME_LOG=1` | Registrar los tiempos de fotograma |
| `RUSTYFILES_LOOK_TRACE=1` | Registrar los tiempos de DirectLook en stderr |
| `RUSTYFILES_MUTE=1` | DirectLook reproduce sin sonido (pruebas) |

La configuración se guarda en `%APPDATA%\rustyfiles\settings.toml`, y la de DirectLook en `directlook.toml`,
junto a él.

</details>

<details>
<summary><b>Cómo se hace</b></summary>

rustyfiles está escrito con herramientas de programación con IA, y no es ningún secreto. Una persona decide qué se construye
y cómo debe comportarse, prueba cada cambio en un PC real (dos monitores, trabajo diario) y descarta lo que no se sostiene.
La IA escribe gran parte del código; no decide qué se publica.

Lo que mantiene esto honesto:

- **Pruebas.** Más de 4,000 pruebas automatizadas se ejecutan antes de subir cada cambio.
- **Mediciones.** Las cifras de velocidad provienen de una prueba integrada en la app (Configuración › Prueba de velocidad),
  así que cualquiera puede comprobarlas en su propio equipo.
- **Informes.** Los errores llegan de quienes usan la app. Cada incidencia recibe una respuesta, la corrección se hace
  en público, y a quien la reportó se le pide que la pruebe antes de cerrarla.
- **El código.** Todo está aquí bajo la licencia MIT, para leerlo y juzgarlo.

Por qué Rust: un gestor de archivos tiene que seguir siendo rápido con 100,000 archivos en una carpeta y nunca puede perder
los datos de nadie. Rust da velocidad sin recolector de basura y descarta familias enteras de fallos antes de publicarse.
rustyfiles es una app nueva escrita desde cero, no una reescritura del Explorador; funciona junto al Explorador
y no cambia nada de Windows en el disco.

</details>

## Estado

rustyfiles es joven, y su autor lo usa cada día en lugar del Explorador. La app de macOS es una primera beta sobre el mismo
núcleo, y Linux llegará después. Los informes de errores y las pull requests son bienvenidos.

## Publicado en

- [AlternativeTo](https://alternativeto.net/software/rustyfiles/about/), como alternativa al Explorador de archivos
- [Awesome GPUI](https://github.com/zed-industries/awesome-gpui#productivity), en la sección Productivity
- [Awesome Rust](https://github.com/rust-unofficial/awesome-rust#file-manager), en la sección de gestores de archivos
- [Scoop](https://scoop.sh), desde su propio bucket (ver [Instalación](#instalación))

## Agradecimientos

rustyfiles se apoya en el trabajo de otras personas, y tomó prestadas muchas ideas en el camino.

[Files](https://github.com/files-community/Files) mostró cómo puede ser un gestor de archivos moderno para Windows. Su
conjunto de funciones fue nuestra lista de verificación, su configuración y su disposición dieron forma a las nuestras, y su
enfoque para reemplazar al Explorador como gestor de archivos predeterminado guio el nuestro. Gracias a la comunidad de Files.

[GPUI](https://gpui.rs), el framework de interfaz del editor [Zed](https://github.com/zed-industries/zed), dibuja cada píxel
de rustyfiles en Windows en la GPU y hace posible una ventana tan rápida. Gracias al equipo de Zed por crearlo y compartirlo,
y a [Longbridge](https://github.com/longbridge/gpui-kit) por gpui-kit, los componentes sobre los que construimos.

DirectLook sigue la idea de Quick Look de macOS, y [QuickLook](https://github.com/QL-Win/QuickLook) de Paddy Xu, que lo llevó
primero a Windows. rarchive aprendió el formato RAR del código fuente de UnRAR de Alexander Roshal.

Construido con [windows-rs](https://github.com/microsoft/windows-rs), [Velopack](https://velopack.io)
(instalación y actualizaciones), los iconos de [Lucide](https://lucide.dev), [zip](https://github.com/zip-rs/zip2),
[sevenz-rust2](https://github.com/hasenbanck/sevenz-rust), [tar](https://github.com/alexcrichton/tar-rs),
[notify](https://github.com/notify-rs/notify) y muchos otros crates. Las imágenes usan
[Bricolage Grotesque](https://github.com/ateliertriay/bricolage) de Atelier Triay,
[Instrument Sans](https://fonts.google.com/specimen/Instrument+Sans) y
[Doto](https://fonts.google.com/specimen/Doto).

## Licencia

MIT, ver [LICENSE](LICENSE). El fork de GPUI conserva la licencia Apache-2.0 de Zed en su propio repositorio.
