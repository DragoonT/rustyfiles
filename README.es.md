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

<img alt="rustyfiles: un administrador de archivos para Windows que nunca se congela. Escrito en Rust. Dibujado como una ventana de rustyfiles sobre una carpeta de 100,000 archivos, abierta en 281 ms" src="docs/readme/cover.png" width="100%">

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest"><img alt="Última versión" src="https://img.shields.io/github/v/release/gelenmedya/rustyfiles?label=download&labelColor=232326&color=ff6a1a"></a>
  <a href="https://github.com/gelenmedya/rustyfiles/releases"><img alt="Descargas" src="https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fgelenmedya%2Frustyfiles%2Fbadges%2Fdownloads.json"></a>
  <img alt="Windows 11, x64 y ARM64" src="https://img.shields.io/badge/Windows_11-x64_%7C_ARM64-3d3d44?labelColor=232326">
  <a href="LICENSE"><img alt="Licencia MIT" src="https://img.shields.io/badge/license-MIT-3d3d44?labelColor=232326"></a>
</p>

rustyfiles es un administrador de archivos para Windows, escrito desde cero en Rust. Está pensado
para reemplazar al Explorador de archivos en el trabajo diario: pestañas, doble panel, archivos
comprimidos que se abren como carpetas y un visor al estilo Quick Look llamado DirectLook.

La ventana nunca espera al disco. El listado, los iconos, las miniaturas, el menú contextual y
cada operación con archivos se ejecutan fuera del hilo de la interfaz, así que un recurso de red
lento o una carpeta con 100,000 archivos no pueden congelarla.

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe"><img alt="Descargar para Windows" src="https://img.shields.io/badge/Download_for_Windows-FF6A1A?style=for-the-badge&logo=windows11&logoColor=white"></a>
  <a href="https://rustyfiles.com"><img alt="Sitio web" src="https://img.shields.io/badge/rustyfiles.com-232326?style=for-the-badge&logo=googlechrome&logoColor=white"></a>
  <a href="https://discord.gg/K2banZ36zJ"><img alt="Únete al Discord" src="https://img.shields.io/badge/Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white"></a>
</p>

<p align="center">
  <a href="https://www.buymeacoffee.com/ozerguzel"><img alt="Invítame a un café" src="https://cdn.buymeacoffee.com/buttons/v2/default-yellow.png" height="44"></a>
</p>

<p align="center">
  <a href="https://alternativeto.net/software/rustyfiles/about/?utm_source=badge&utm_medium=referral">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://alternativeto.net/static/badges/badge-compact-dark.svg">
      <img alt="rustyfiles en AlternativeTo" src="https://alternativeto.net/static/badges/badge-compact-light.svg" height="54">
    </picture>
  </a>
</p>

Gratis, sin cuenta, sin anuncios. Descarga, haz doble clic y listo: ver [Instalación](#instalación).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/readme/projects-dark.png">
  <img alt="rustyfiles mostrando una carpeta Proyectos en vista de detalles, con pestañas, una barra de ruta y carpetas fijadas en la barra lateral; la ventana es de cristal sobre el fondo de escritorio" src="docs/readme/projects-light.png" width="100%">
</picture>

<sub>Las imágenes muestran el fondo experimental HyperGlass (Configuración › Apariencia). El fondo predeterminado es Mica, como en el Explorador.</sub>

## Qué tan rápido

<img alt="Una prueba de velocidad dibujada como una lista de rustyfiles: abrir una carpeta de 100,000 archivos tarda 281 ms en rustyfiles y 2,318 ms en el Explorador de archivos (8 veces más rápido); una carpeta de 2,000 imágenes, 278 ms contra 2,177 ms (8 veces); copiar 10,000 archivos pequeños, 1.8 s contra 25.6 s (14 veces); extraer un ZIP de 10,000 archivos, 3.6 s contra 32.4 s (9 veces)" src="docs/readme/speed.png" width="100%">

### Abrir una carpeta

Tiempo desde que se inicia la app hasta que la lista de la carpeta deja de moverse, en una ventana
nueva, con la carpeta ya en la caché de archivos del sistema. Mediana de tres ejecuciones tras un
calentamiento, medida con la prueba de velocidad integrada en rustyfiles (Configuración › Prueba de
velocidad): cualquiera puede ejecutarla en su propia máquina.

| Carpeta | rustyfiles | Explorador de archivos | [Files](https://files.community) |
|---|--:|--:|--:|
| Carpeta vacía | 281 ms | 1,006 ms | 813 ms |
| 1,000 archivos | 284 ms | 2,214 ms | 1,893 ms |
| 10,000 archivos | 286 ms | 2,221 ms | 1,053 ms |
| 100,000 archivos | 281 ms | 2,318 ms | 4,069 ms |
| 2,000 imágenes (miniaturas) | 278 ms | 2,177 ms | 1,075 ms |
| Memoria tras listar 10,000 archivos | 39 MB | 69 MB | 204 MB |

Medido en una misma máquina (Ryzen 7 9800X3D, SSD NVMe, Windows 11) el 2026-11-16, con rustyfiles
0.8.6 y Files 4.3 (0.8.5 tardó unos 540 ms en la misma prueba: su primera pestaña reproducía una
animación de apertura que 0.8.6 ya no reproduce al iniciar). rustyfiles y el Explorador iniciaron
cada uno un proceso nuevo. Files ya estaba ejecutándose en segundo plano, así que sus tiempos no
son los de un arranque en frío; además, rellena su lista por pasos, y el tiempo cuenta hasta que la
imagen deja de cambiar. La memoria es el conjunto de trabajo privado, la cifra que muestra el
Administrador de tareas; en el caso de Files, es la de su proceso en ejecución.

Con «seguir ejecutándose en segundo plano» activado, una ventana nueva de rustyfiles no inicia
ningún proceso: en una medición anterior (2026-11-06, con una versión anterior) mostró una carpeta
ya listada en 93–168 ms.

### Una carpeta de 100,000 archivos que no deja de cambiar

Se abre la carpeta, se selecciona un archivo y luego, durante 5 segundos, se renombran en segundo
plano unos 1,000 archivos por segundo:

| | |
|---|--:|
| Desde el inicio hasta una carpeta listada (100,001 archivos) | 315 ms |
| La lista sigue los cambios | cada ~750 ms |
| Relectura de la carpeta (hilo en segundo plano) | ~70 ms |
| Trabajo en el hilo de la interfaz por actualización | 7–8 ms |
| Selección y cursor | se mantienen sobre el archivo seleccionado |

La ventana nunca espera a la carpeta: la relectura se hace fuera del hilo de la interfaz, y la
actualización en sí cuesta menos que un fotograma a 120 Hz. Mídelo tú mismo con
`RUSTYFILES_TRACE_LIVE=1` y `RUST_LOG=warn,rf_app=info`.

Tiempos de fotograma mientras ocurre: la misma carpeta, desplazada hacia abajo y hacia arriba con la
rueda del ratón durante 7.7 s, con la selección (y por tanto el panel de vista previa) cambiando cada
medio segundo, mientras unos 680 archivos por segundo se renombran en segundo plano. Pantalla de 200 Hz.

| | mediana | p95 | p99 | peor |
|---|--:|--:|--:|--:|
| Coste del fotograma en el hilo de la interfaz | 2.1 ms | 4.6 ms | 7.9 ms | 16.8 ms |
| Tiempo entre fotogramas | 3.3 ms | 5.3 ms | 9.9 ms | 17.3 ms |

2,182 fotogramas en 7.7 s; una pausa más larga que un fotograma a 60 Hz (16.7 ms). Registra los tuyos
con `GPUI_FRAME_LOG=frames.tsv` (una línea por fotograma: su inicio y su coste, en ms).

### Pestaña nueva en una ventana abierta

| Carpeta | Tiempo hasta una lista estable |
|---|--:|
| 1,000 archivos | ~48 ms |
| 10,000 archivos | ~46 ms |
| 100,000 archivos | ~129 ms |

Cada pestaña adicional cuesta unos 3 MB de memoria (1 pestaña: ~32 MB, 25 pestañas: ~103 MB).

### Buscar por nombre de archivo

rustyfiles mantiene un índice de nombres de las unidades locales y responde las búsquedas desde él.
Búsqueda de «rapor» en todas las subcarpetas de una carpeta de usuario con 3.6 millones de archivos
y carpetas, medida hasta que los resultados en pantalla dejaron de cambiar:

| | Tiempo |
|---|--:|
| rustyfiles 0.4 | **0.38 s** |
| Explorador de archivos | 1.8 s (su mejor de 4 ejecuciones; las demás tardaron 11–32 s) |

El índice guarda cada nombre distinto una sola vez: una unidad con 4.8 millones de entradas pero
490 mil nombres distintos ocupa 61 MB. Un vigilante de carpetas lo mantiene al día mientras
rustyfiles se ejecuta. Entre ejecuciones, el diario de cambios de NTFS (que se lee sin derechos de
administrador) indica qué carpetas cambiaron, así que al iniciar solo se vuelven a listar esas en
lugar de recorrer las unidades: 0.2 s para C: aquí, donde un recorrido tarda 30 s. Los datos de
aplicaciones, las cachés, las carpetas `node_modules` y `.git` quedan fuera del índice; una búsqueda
iniciada dentro de una de ellas recorre el disco:

`*.dll` en `C:\Windows`: 342,492 elementos en 116,851 carpetas, con la caché de archivos caliente.

| Método | Tiempo |
|---|--:|
| rustyfiles | **7.8 s** |
| `dir /s /b` | 53 s |
| PowerShell `Get-ChildItem -Recurse` | 105 s |

rustyfiles busca en seis hilos de prioridad baja, así que el resto de la máquina sigue respondiendo.
No sigue las uniones (junctions), por eso encuentra 454 duplicados menos.

### Vista previa con Space (DirectLook)

Tiempo desde la pulsación de la tecla hasta que el contenido aparece en pantalla, con la app ya abierta:

| Archivo | Contenido listo |
|---|--:|
| JPEG, 3840 × 2160 | 24–86 ms |
| Código fuente, con resaltado de sintaxis | 23–44 ms |
| Archivo 7z (contenido listado) | 19–31 ms |
| MP3 (etiquetas, letra) | 27–32 ms |
| Video H.264, 1080p | primer fotograma en 146–185 ms |

### Archivos comprimidos

Lectura de RAR con [rarchive](https://github.com/gelenmedya/rarchive), nuestro propio motor, frente
al UnRAR oficial (decodificar y verificar cada CRC, archivos en la caché del sistema):

| Archivo | UnRAR | rarchive |
|---|--:|--:|
| 1.3 GB, diccionario de 32 MB, 195 archivos | 2.79 s | **0.28 s** (10×) |
| 400 MB sólido | 0.95 s | **0.21 s** (4.5×) |
| 400 MB, 520 archivos | 1.03 s | **0.063 s** (16×) |

### Copiar y extraer

La misma prueba de velocidad, nuestros motores frente a los de Windows (rustyfiles 0.8.6, 2026-11-16):

| | Windows | rustyfiles |
|---|--:|--:|
| Copiar 10,000 archivos pequeños en 100 carpetas (41 MB) | 25.6 s | **1.8 s** (14×) |
| Copiar un archivo de 1 GB | 0.61 s | 0.44 s |
| Extraer un ZIP de 10,000 archivos | 32.4 s | **3.6 s** (9×) |

Copiar usa su propio motor en paralelo, que reparte los archivos pequeños entre carpetas porque los
hilos que escriben en una misma carpeta se esperan unos a otros. Un archivo grande único está
limitado por el disco, no por el motor (en una ejecución anterior, Windows fue el más rápido de los
dos por unos pocos ms). Buscar por nombre en las carpetas de la prueba (123,000 entradas) tardó 55 ms.

También gestiona los conflictos de nombres por sí mismo (reemplazar, omitir o conservar ambos, para
todos o archivo por archivo), pausa y reanuda, puede verificar cada copia byte a byte y puede limitar
su velocidad.

Todas las cifras: una misma máquina (Ryzen 7 9800X3D, SSD NVMe, Windows 11), noviembre de 2026. Abrir
una carpeta, copiar y extraer proceden de la prueba de velocidad integrada (Configuración › Prueba de
velocidad); los demás scripts de prueba están en el repositorio (`crates/rf-win/examples/copybench.rs`
para copiar).

## Qué hace

**Navegación.** Pestañas que puedes arrastrar para reordenar o sacar a una ventana nueva. Guarda las
pestañas de una ventana, incluidos los paneles divididos, como una sesión con nombre y ábrela de nuevo
más tarde; su nombre aparece junto a las pestañas. Una pestaña nueva abre Inicio o la carpeta en la
que estás (Configuración › General). Doble panel. Vistas de detalles, cuadrícula, lista y columnas
(la lista puede llenarse columna por columna, como en el Explorador), agrupación, panel de vista
previa, búsqueda, etiquetas de color, la Estantería (Shelf), bibliotecas, recursos compartidos de red
y FTP. Una columna de estado de Git. Tamaños de carpeta a demanda: haz clic en «Calcular tamaño»; se
recuerdan entre reinicios y pueden mostrar el tamaño en disco. «Mostrar uso de disco» en una carpeta
o unidad enumera lo que ocupa el espacio, de mayor a menor, junto a un mapa de árbol en el que puedes
hacer clic (C:\Windows, 341 mil entradas: 26 MB en memoria). Las carpetas virtuales como Bibliotecas
y Galería se abren en rustyfiles, y los dispositivos que Windows lista en Este equipo (teléfonos por
USB o Phone Link) aparecen en Dispositivos en la página de Inicio; esa parte todavía no se ha probado
con un teléfono real.

**Archivos.** Deshacer y rehacer para renombrar, copiar, mover, eliminar a la Papelera de reciclaje y
aplanar carpetas. Renombrado masivo. Duplicar, copiar o mover al otro panel, y pegar texto o una
imagen copiados como un archivo nuevo. El menú contextual completo del Explorador con Enviar a,
arrastrar y soltar con el Explorador, el escritorio, los navegadores y Outlook, y zonas de destino en
pestañas, carpetas fijadas, carpetas y programas. Envía archivos a tus otros equipos con
[Ferry](#ferry), y descarga con [HyperDownload](#hyperdownload).

**Archivos comprimidos.** ZIP, 7z, RAR, la familia tar e ISO se abren como carpetas, incluidos los ZIP,
7z y RAR protegidos con contraseña. Añade, elimina y renombra dentro de ZIP, 7z y tar. Crea ZIP, 7z y
tar.zst usando todos los núcleos. La lectura la hace [rarchive](https://github.com/gelenmedya/rarchive),
nuestro motor en Rust puro (también [en crates.io](https://crates.io/crates/rarchive) para otros
proyectos en Rust): descodifica un archivo WinRAR de 1.3 GB diez veces más rápido que UnRAR.

**Windows.** Opcionalmente, el administrador de archivos predeterminado: las carpetas,
<kbd>Win</kbd>+<kbd>E</kbd>, Este equipo y la Papelera de reciclaje se abren en rustyfiles.
Opcionalmente, los diálogos Abrir, Guardar y Seleccionar carpeta de otras apps también se abren en
rustyfiles (Configuración › Avanzado), y un botón de chincheta allí guarda una carpeta de inicio para
cada app. Las imágenes y los videos abiertos en Fotos u otra app de la Tienda llegan con su carpeta,
así que sus botones Anterior y Siguiente funcionan. Las rutas `/select,` y `shell:` del Explorador
funcionan. Mica, modo oscuro y el color de énfasis del sistema. Cuando un juego o un render satura la
GPU, el dibujo pasa a la CPU y vuelve a la GPU sin parpadeo.

**Comandos en la barra de direcciones.** Como en el Explorador: pulsa <kbd>Ctrl</kbd>+<kbd>L</kbd>,
escribe `cmd`, `powershell`, `wt` o `code .` y pulsa <kbd>Enter</kbd>. El texto se toma primero como
un lugar (una ruta, una unidad, `%VAR%`, `shell:`); una carpeta o archivo con ese nombre dentro de la
carpeta abierta siempre gana. Solo cuando no existe ese lugar, la primera palabra se busca como
programa, igual que en el cuadro Ejecutar, y se inicia en la carpeta abierta con el resto como
argumentos. No se inicia nada desde una sugerencia de autocompletado y no se piden derechos de
administrador. Un interruptor en Configuración › Archivos y carpetas › Comportamientos lo desactiva.

**Organización.** Reglas para una carpeta: condiciones por nombre, extensión, tipo, tamaño y
antigüedad, y una acción (mover, mover a una carpeta creada desde una plantilla, renombrar o la
Papelera de reciclaje). Una ejecución manual muestra cada paso en una vista previa y no mueve nada
antes de Aplicar, y una ejecución puede deshacerse. Nada se elimina para siempre y nada se
sobrescribe. Las reglas que se ejecutan solas están desactivadas por defecto.

**Búsqueda.** La búsqueda rápida (<kbd>Alt</kbd>+<kbd>Space</kbd>) encuentra apps, archivos y carpetas,
marcadores del navegador y las páginas de configuración de Windows. Lo que abres con frecuencia
aparece primero, y lo que elegiste antes para las mismas letras es la primera fila.

**Idiomas.** 49, según el idioma de visualización de Windows (o elige uno en Configuración › General).
La interfaz está escrita en turco e inglés; los demás idiomas partieron de la redacción de la
comunidad de Files y se traducen a máquina a partir de ahí. El árabe, el hebreo y el persa están
traducidos, y los nombres y textos de derecha a izquierda se leen de derecha a izquierda (gracias a
@ialhabbal); la interfaz en sí todavía no está reflejada. Las correcciones son bienvenidas: cada
idioma es un archivo JSON en `crates/rf-app/locales/`.

<img alt="rustyfiles mostrando una carpeta con los fondos de pantalla de Windows en vista de cuadrícula, uno de ellos seleccionado y mostrado en el panel de vista previa a la derecha, sobre el fondo HyperGlass" src="docs/readme/grid.png" width="100%">

## DirectLook

Selecciona un archivo y pulsa <kbd>Space</kbd>. DirectLook abre una ventana de vista previa: video y
audio (con su propio reproductor), imágenes, texto y código, el contenido de los archivos comprimidos,
y una tarjeta de detalles para todo lo demás. Las teclas de flecha se mueven por la carpeta mientras
la ventana sigue abierta.

Se ejecuta como programa independiente, `directlook.exe`, así que un video pesado nunca pesa sobre el
administrador de archivos y ninguno de los dos puede tumbar al otro. Funciona en rustyfiles y, si lo
activas, también en el Explorador y en el escritorio. La configuración está en Configuración › DirectLook.

## Ferry

Envía archivos y carpetas a rustyfiles en tus otros equipos de la misma red, como AirDrop:
clic derecho › **Enviar con Ferry**.

<img alt="Configuración › Ferry con dos dispositivos cercanos, un PC con Windows emparejado con su velocidad medida y un MacBook con una velocidad estimada; una notificación de Windows pregunta si aceptar archivos del MacBook, y la barra de estado muestra una transferencia al 61 por ciento" src="docs/readme/ferry.png" width="100%">

<sub>Dibujo de Configuración › Ferry con dispositivos inventados, con las palabras y la disposición propias de la app.</sub>

- Cifrado de extremo a extremo (QUIC con TLS 1.3), directo de dispositivo a dispositivo: sin servidor,
  sin cuenta, nada sale de tu red.
- Empareja tus dispositivos como emparejas Bluetooth: ambos muestran el mismo código de seis dígitos.
  Los archivos de un dispositivo emparejado pueden llegar sin preguntar; cualquier otro se pregunta
  primero, en una notificación.
- Tú eliges quién puede ver tu PC: nadie, tus dispositivos emparejados (la opción predeterminada) o
  todos durante diez minutos. Mientras Configuración › Ferry está abierta, todos pueden verlo, para
  que un dispositivo nuevo lo encuentre.
- Una transferencia interrumpida continúa desde donde se detuvo, y cada archivo se verifica antes de
  tomar su nombre. Los archivos recibidos se marcan como descargados, así que SmartScreen pregunta
  antes de que se ejecute un programa entre ellos.
- Cada dispositivo muestra una velocidad estimada, y **Probar velocidad** la mide en ambos sentidos
  en unos pocos segundos.
- La app de macOS, que llegará pronto, habla el mismo protocolo, documentado en
  [docs/nearby-protocol.md](docs/nearby-protocol.md).

Ferry está desactivado hasta que lo actives en Configuración › Ferry. El Firewall de Windows puede
preguntar por él: permítelo para redes privadas.

## Experimental: HyperGlass, el Dock y el escritorio

Estas funciones están desactivadas por defecto y son tempranas. Se construyeron y probaron en un PC
con dos monitores, y algunas partes se han usado muy poco. Espera asperezas y, por favor, informa de
lo que encuentres.

<img alt="Un escritorio con el Dock de rustyfiles abajo (tres islas: Inicio y búsqueda, las apps fijadas y en ejecución, la bandeja y el reloj), el panel Inicio del Dock abierto a la izquierda con apps fijadas y archivos recientes, HyperSearch en el centro con las apps encontradas para adobe, y el centro de control a la derecha con Wi-Fi, Bluetooth, Ethernet, la canción que suena, volumen y brillo" src="docs/readme/shell.png" width="100%">

<sub>Inicio, HyperSearch y el centro de control, compuestos a partir de tres capturas del mismo escritorio (no se abren todos a la vez).</sub>

- **HyperGlass** (Configuración › Apariencia) es un quinto fondo: el fondo de la ventana es un cristal
  que curva y esmerila lo que hay detrás.
- **El Dock** (Configuración › Dock, un solo interruptor) pone tres islas de cristal en la parte
  inferior de cada monitor en lugar de la barra de tareas de Windows: un panel Inicio y búsqueda
  propios, las apps en ejecución y fijadas con vistas previas de ventanas, los iconos de la bandeja,
  un centro de control y un panel de notificaciones. Mientras está activado, la barra de tareas de
  Windows se oculta, un toque a la tecla Windows abre el Inicio del Dock y <kbd>Alt</kbd>+<kbd>Tab</kbd>
  abre su selector. Si desactivas el Dock, vuelven los de Windows.
- **El escritorio** dibujado por rustyfiles viene con el Dock: tu fondo de pantalla e iconos, con una
  opción para esquinas redondeadas.

El Explorador sigue ejecutándose debajo de todo esto; no se elimina ni se reemplaza nada de Windows
en el disco.

### Brillo para cada monitor

Windows solo tiene un control de brillo para la pantalla de un portátil. El centro de control del Dock
tiene uno para **cada monitor**: activa Configuración › Dock y luego haz clic en el icono de red o de
volumen a la derecha del Dock. Los monitores externos se ajustan por **DDC/CI**, el estándar que casi
todos los monitores de escritorio de los últimos quince años hablan por DisplayPort, HDMI y DVI. Si un
monitor no tiene control deslizante, comprueba que DDC/CI esté activado en el propio menú del monitor;
algunas bases, conmutadores KVM y adaptadores no lo transmiten. La pantalla de un portátil se ajusta
de la misma forma que lo hace el control deslizante propio de Windows (aún no probado en un portátil).

## HyperDownload

El administrador de descargas propio de rustyfiles, un complemento: Configuración › Complementos ›
HyperDownload › **Instalar**. Es un programa aparte ([código fuente](https://github.com/gelenmedya/hyperdownload),
MIT), que solo se descarga cuando haces clic en Instalar, cuyo SHA-256 se verifica antes, y puede
eliminarse en cualquier momento.

- **Descargar desde un enlace** en el menú contextual de una carpeta: un archivo por muchas conexiones
  a la vez, pausado y reanudado donde se detuvo, también tras un reinicio. Pega un enlace en una
  carpeta y rustyfiles te ofrece descargarlo allí.
- **Torrents y enlaces magnet**, con un diálogo para elegir qué archivos de un torrent quieres. Un
  archivo `.torrent` se descarga desde su propio menú.
- **Una página de Descargas** con todas las descargas, sus piezas y su progreso, y las terminadas;
  el progreso también aparece en el Centro de estado.
- **Ver mientras se descarga:** un video se reproduce en DirectLook antes de terminar.

## Instalación

Instalar lleva tres clics y ningún conocimiento técnico:

1. **[Descarga rustyfiles-win-Setup.exe](https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe)** (unos 50 MB).
2. Ábrelo. Si Windows muestra «Windows protegió su PC», haz clic en **Más información** y luego en
   **Ejecutar de todos modos**. (El instalador todavía no tiene firma de código; esa es la única razón
   de la advertencia. Los instaladores de cada versión se analizan en
   [VirusTotal](https://www.virustotal.com): los enlaces a los informes están al final de sus notas
   de versión.)
3. Listo. rustyfiles se abre solo y aparece en tu menú Inicio.

No hace falta contraseña de administrador ni hay nada que configurar. Las actualizaciones se descargan
en silencio en segundo plano y se aplican la próxima vez que abras rustyfiles.

**Con [Scoop](https://scoop.sh):**

```powershell
scoop bucket add rustyfiles https://github.com/gelenmedya/scoop-rustyfiles
scoop install rustyfiles/rustyfiles
```

**¿Quieres que las carpetas y <kbd>Win</kbd>+<kbd>E</kbd> se abran en rustyfiles?** Activa
Configuración › Avanzado › Establecer como administrador de archivos predeterminado. Puedes
desactivarlo cuando quieras.

**Desinstala** como cualquier app, desde Configuración de Windows › Aplicaciones. Todo lo que
rustyfiles cambió vuelve al Explorador de archivos.

**¿Los diálogos Abrir/Guardar de otras apps fallan o se cierran** después de que se eliminaran los
archivos de rustyfiles sin desinstalar (un antivirus, una actualización rota)? Volver a iniciar
rustyfiles lo arregla. Si rustyfiles ya no arranca, este comando de PowerShell devuelve a todas las
apps el diálogo propio de Windows:

```powershell
Remove-Item -Recurse "HKCU:\Software\Classes\CLSID\{DC1C5A9C-E88A-4dde-A5A1-60F82A20AEF7}", "HKCU:\Software\Classes\CLSID\{C0B4E2F3-BA21-4773-8DBA-335EC946EB8B}"
```

¿Prefieres no usar instalador? Hay un zip portable en la página de
[Releases](https://github.com/gelenmedya/rustyfiles/releases), y también un instalador ARM64
(`rustyfiles-win-arm64-Setup.exe`). Requiere Windows 11 (Windows 10 no está probado). Una versión
para macOS llegará pronto, y Linux está previsto.

**¿Preguntas o ideas?** Pásate a saludar en [Discord](https://discord.gg/K2banZ36zJ), o visita
[rustyfiles.com](https://rustyfiles.com).

### Compilar desde el código fuente

Necesitas [Rust](https://rustup.rs) 1.95 o más reciente y las Build Tools de Visual Studio con la
carga de trabajo «Desarrollo para el escritorio con C++» (una dependencia, zstd, compila código C).

```powershell
git clone https://github.com/gelenmedya/rustyfiles
cd rustyfiles
.\scripts\install.ps1            # compila y luego instala para tu usuario
.\scripts\install.ps1 -Default   # además lo hace el administrador de archivos predeterminado
.\scripts\install.ps1 -Uninstall
```

`cargo run --release -p rf-app` lo inicia sin instalarlo.

## Teclado

| Teclas | Acción |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>T</kbd> / <kbd>Ctrl</kbd>+<kbd>W</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd> | Pestaña nueva, cerrar pestaña, reabrir la pestaña cerrada |
| <kbd>Ctrl</kbd>+<kbd>L</kbd> o <kbd>Alt</kbd>+<kbd>D</kbd> | Barra de direcciones |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> | Buscar (<kbd>Enter</kbd> también busca en las subcarpetas) |
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

Cada atajo se puede cambiar en Configuración › Acciones: haz clic en un comando y luego pulsa las
nuevas teclas.

## Estructura del proyecto

| Crate | Qué contiene |
|---|---|
| `crates/rf-app` | La interfaz, y los dos programas: `rustyfiles` y `directlook` (`src/look/`) |
| `crates/rf-win` | Integración con Windows y con el shell, el motor de operaciones con archivos |
| `crates/rf-core` | El modelo: ubicaciones, historial, entradas |
| `crates/rf-archive` | Operaciones con archivos comprimidos: explorar, extraer, crear, editar |
| `crates/rf-player` | El reproductor de video y audio de DirectLook |
| `crates/rustdecoder` | Nuestro propio motor de video, en desarrollo: MP4/MOV (también fragmentado) y MKV/WebM; H.264, HEVC, VP9 y AV1 decodificados en la tarjeta gráfica mediante D3D11 Video, con resultados idénticos bit a bit a ffmpeg / dav1d en sus pruebas; ProRes y DNxHR decodificados en la CPU; conversión de color, audio (PCM, AAC, FLAC, Opus, MP3) y sincronía A/V. Asumirá la reproducción de video de DirectLook ([diseño](docs/design/rustdecoder.md)) |

La interfaz está construida con [GPUI](https://gpui.rs), el framework detrás del editor Zed. Su capa
de Windows es una bifurcación que añade el cambio GPU/CPU:
[gelenmedya/gpui-pre-windows](https://github.com/gelenmedya/gpui-pre-windows).

<details>
<summary>Variables de entorno</summary>

| Variable | Efecto |
|---|---|
| `RUSTYFILES_RENDERER` = `gpu`, `cpu` o `auto` | Forzar dónde se dibuja la ventana |
| `RUSTYFILES_CONFIG_DIR` = carpeta | Guardar la configuración en otra carpeta (uso portátil) |
| `RUSTYFILES_MULTI_INSTANCE=1` | Permitir varias copias a la vez (pruebas) |
| `RUSTYFILES_FRAME_LOG=1` | Registrar los tiempos de fotograma |
| `RUSTYFILES_LOOK_TRACE=1` | Registrar en stderr los tiempos de DirectLook |
| `RUSTYFILES_MUTE=1` | DirectLook reproduce sin sonido (pruebas) |

La configuración se guarda en `%APPDATA%\rustyfiles\settings.toml`, y la de DirectLook en
`directlook.toml`, junto a él.

</details>

## Cómo se hace

rustyfiles se escribe con herramientas de programación con IA, y eso no es ningún secreto. Una persona
desarrolladora decide qué se construye y cómo debe comportarse, prueba cada cambio en un PC real (dos
monitores, trabajo diario) y descarta lo que no se sostiene. La IA escribe buena parte del código; no
decide qué se publica.

Lo que mantiene esto honesto:

- **Pruebas.** Más de 3,000 pruebas automáticas se ejecutan antes de subir cada cambio.
- **Mediciones.** Las cifras de velocidad de arriba salen de una prueba integrada en la app
  (Configuración › Prueba de velocidad), así que cualquiera puede comprobarlas en su propia máquina.
- **Reportes.** Los errores llegan de quienes usan la app. Cada incidencia recibe respuesta, la
  corrección se hace a la vista de todos, y a quien la reportó se le pide que la pruebe antes de que
  se cierre.
- **El código.** Todo está aquí bajo la licencia MIT, para leerlo y juzgarlo.

Por qué Rust: un administrador de archivos tiene que seguir siendo rápido con 100,000 archivos en una
carpeta y nunca puede perder los datos de nadie. Rust da velocidad sin recolector de basura y descarta
clases enteras de fallos antes de publicar. Y rustyfiles es una app nueva escrita desde cero, no una
reescritura del Explorador; se ejecuta junto al Explorador y no cambia nada de Windows en el disco.

## Estado

rustyfiles es joven, y quien lo desarrolla lo usa cada día en lugar del Explorador. Una hoja de ruta y
los informes de pruebas llegarán en `docs/`. La versión para macOS está en camino, construida sobre el
mismo núcleo, y Linux vendrá después.

Los reportes de errores y los pull requests son bienvenidos.

## Aparece en

- [AlternativeTo](https://alternativeto.net/software/rustyfiles/about/), como alternativa al Explorador de archivos
- [Scoop](https://scoop.sh), desde su propio bucket (ver [Instalación](#instalación))

## Agradecimientos

rustyfiles se apoya en el trabajo de otras personas y tomó prestadas muchas ideas por el camino.

[Files](https://github.com/files-community/Files) mostró lo que puede ser un administrador de archivos
moderno para Windows. Su lista de funciones fue nuestra lista de control, su configuración y su diseño
dieron forma a los nuestros, y su enfoque para reemplazar al Explorador como administrador de archivos
predeterminado guió el nuestro. Gracias a la comunidad de Files.

[GPUI](https://gpui.rs), el framework de interfaz del editor [Zed](https://github.com/zed-industries/zed),
dibuja cada píxel de rustyfiles en la GPU y hace posible una ventana tan rápida. Gracias al equipo de
Zed por construirlo y compartirlo, y a [Longbridge](https://github.com/longbridge/gpui-kit) por gpui-kit,
los componentes sobre los que construimos.

DirectLook sigue la idea del Quick Look de macOS, y de [QuickLook](https://github.com/QL-Win/QuickLook),
de Paddy Xu, que lo llevó primero a Windows. rarchive aprendió el formato RAR del código fuente de
UnRAR de Alexander Roshal.

Construido con [windows-rs](https://github.com/microsoft/windows-rs),
[Velopack](https://velopack.io) (instalación y actualizaciones), iconos de [Lucide](https://lucide.dev),
[zip](https://github.com/zip-rs/zip2), [sevenz-rust2](https://github.com/hasenbanck/sevenz-rust),
[tar](https://github.com/alexcrichton/tar-rs), [notify](https://github.com/notify-rs/notify)
y muchos otros crates. Las imágenes usan [Bricolage Grotesque](https://github.com/ateliertriay/bricolage)
de Atelier Triay, [Instrument Sans](https://fonts.google.com/specimen/Instrument+Sans) y
[Doto](https://fonts.google.com/specimen/Doto).

## Licencia

MIT, ver [LICENSE](LICENSE). El fork de GPUI conserva la licencia Apache-2.0 de Zed en su propio
repositorio.
