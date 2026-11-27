<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.zh-CN.md">简体中文</a> ·
  <a href="README.zh-TW.md">繁體中文</a> ·
  <a href="README.ja.md">日本語</a> ·
  <a href="README.de.md">Deutsch</a> ·
  <a href="README.es.md">Español</a> ·
  <b>Português (Brasil)</b> ·
  <a href="README.ru.md">Русский</a> ·
  <a href="README.tr.md">Türkçe</a>
</p>

<p align="center"><sub>Esta é uma tradução; a referência é o <a href="README.md">README</a> em inglês.</sub></p>

<img alt="rustyfiles: um gerenciador de arquivos para Windows e macOS que nunca trava. Escrito em Rust. Desenhado como uma janela do rustyfiles sobre uma pasta de 100.000 arquivos, aberta em 281 ms" src="docs/readme/cover.png" width="100%">

<p align="center">
  <b>Um gerenciador de arquivos que nunca espera pelo disco.</b><br>
  Abas, painel duplo, arquivos compactados que se abrem como pastas e uma visualização com <kbd>Space</kbd>.<br>
  Escrito do zero em Rust, para Windows e agora macOS.
</p>

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest"><img alt="Última versão" src="https://img.shields.io/github/v/release/gelenmedya/rustyfiles?label=download&labelColor=232326&color=ff6a1a"></a>
  <a href="https://github.com/gelenmedya/rustyfiles/releases"><img alt="Downloads" src="https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fgelenmedya%2Frustyfiles%2Fbadges%2Fdownloads.json"></a>
  <img alt="Windows 11, x64 e ARM64" src="https://img.shields.io/badge/Windows_11-x64_%7C_ARM64-3d3d44?labelColor=232326">
  <img alt="macOS 26 em Apple silicon e Intel, beta" src="https://img.shields.io/badge/macOS_26-Apple_silicon_and_Intel_(beta)-3d3d44?labelColor=232326">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest"><img alt="Cada versão é verificada no VirusTotal: os relatórios estão linkados no fim das notas dela" src="https://img.shields.io/badge/VirusTotal-every_release_scanned-3d3d44?labelColor=232326"></a>
  <a href="LICENSE"><img alt="Licença MIT" src="https://img.shields.io/badge/license-MIT-3d3d44?labelColor=232326"></a>
</p>

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe"><img alt="Baixar para Windows" src="https://img.shields.io/badge/Download_for_Windows-FF6A1A?style=for-the-badge&logo=windows11&logoColor=white"></a>
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-mac.dmg"><img alt="Baixar para macOS (beta)" src="https://img.shields.io/badge/Download_for_macOS_(beta)-232326?style=for-the-badge&logo=apple&logoColor=white"></a>
  <br>
  <a href="https://rustyfiles.com"><img alt="Site" src="https://img.shields.io/badge/rustyfiles.com-232326?style=flat-square&logo=googlechrome&logoColor=white"></a>
  <a href="https://discord.gg/K2banZ36zJ"><img alt="Entre no Discord" src="https://img.shields.io/badge/Discord-5865F2?style=flat-square&logo=discord&logoColor=white"></a>
  <a href="https://www.buymeacoffee.com/ozerguzel"><img alt="Me pague um café" src="https://img.shields.io/badge/Buy_me_a_coffee-FFDD00?style=flat-square&logo=buymeacoffee&logoColor=black"></a>
</p>

<table>
  <tr>
    <td width="33%" valign="top">
      <b>Nunca espera pelo disco</b><br>
      Listagem, ícones, miniaturas, o menu de contexto e cada cópia rodam fora da thread da janela.
      Um compartilhamento de rede lento ou uma pasta de 100.000 arquivos não consegue travá-lo.
    </td>
    <td width="33%" valign="top">
      <b>O trabalho do Explorador, mais rápido</b><br>
      Abas, painel duplo, arquivos compactados como pastas, um índice de nomes para busca instantânea, renomeação em lote, desfazer,
      e um motor de cópia próprio: 14 vezes mais rápido em arquivos pequenos.
    </td>
    <td width="33%" valign="top">
      <b>Gratuito e seu</b><br>
      Sem conta, sem anúncios, nada enviado para lugar nenhum. Licença MIT, cada linha aqui para ler.
      As atualizações são instaladas silenciosamente em segundo plano.
    </td>
  </tr>
</table>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/readme/projects-dark.png">
  <img alt="rustyfiles mostrando uma pasta Projetos na visualização de detalhes, com abas, barra de caminho e pastas fixadas na barra lateral; a janela é vidro sobre o papel de parede da área de trabalho" src="docs/readme/projects-light.png" width="100%">
</picture>

<sub>Windows, com o fundo experimental HyperGlass (Configurações › Aparência). O padrão é o Mica, como no Explorer.</sub>

## Velocidade

<img alt="Um teste de velocidade desenhado como uma lista do rustyfiles: abrir uma pasta de 100.000 arquivos leva 281 ms no rustyfiles e 2,318 ms no Explorador de Arquivos (8 vezes mais rápido); uma pasta de 2.000 imagens, 278 ms contra 2,177 ms (8 vezes); copiar 10.000 arquivos pequenos, 1.8 s contra 25.6 s (14 vezes); extrair um ZIP de 10.000 arquivos, 3.6 s contra 32.4 s (9 vezes)" src="docs/readme/speed.png" width="100%">

Uma pasta de 100.000 arquivos abre em 281 ms (Explorador de Arquivos: 2.3 s). Os números vêm do teste de
velocidade integrado ao app, Configurações › Teste de velocidade, para que você possa conferi-los no seu próprio PC.

<details>
<summary><b>Todas as medições e como foram feitas</b></summary>

#### Abrindo uma pasta

Tempo desde o início do app até a lista da pasta ficar parada, em uma janela nova, com a pasta já
no cache de arquivos do sistema. Mediana de três execuções após um aquecimento.

| Pasta | rustyfiles | Explorador de Arquivos | [Files](https://files.community) |
|---|--:|--:|--:|
| Pasta vazia | 281 ms | 1,006 ms | 813 ms |
| 1.000 arquivos | 284 ms | 2,214 ms | 1,893 ms |
| 10.000 arquivos | 286 ms | 2,221 ms | 1,053 ms |
| 100.000 arquivos | 281 ms | 2,318 ms | 4,069 ms |
| 2.000 imagens (miniaturas) | 278 ms | 2,177 ms | 1,075 ms |
| Memória após listar 10.000 arquivos | 39 MB | 69 MB | 204 MB |

Medido em uma máquina (Ryzen 7 9800X3D, SSD NVMe, Windows 11) em 2026-11-16, com o rustyfiles
0.8.6 e o Files 4.3. O rustyfiles e o Explorador iniciaram cada um um processo novo. O Files já estava
rodando em segundo plano, então seus tempos não são de um início a frio; ele também preenche a lista em
etapas, e o tempo conta até a imagem parar de mudar. Memória é o conjunto de trabalho privado, o número que o
Gerenciador de Tarefas mostra. Com "continuar executando em segundo plano" ligado, uma nova janela do rustyfiles não
inicia nenhum processo e mostrou uma pasta listada em 93–168 ms (2026-11-06, uma versão anterior).

#### Uma pasta de 100.000 arquivos que não para de mudar

A pasta aberta, um arquivo selecionado, e cerca de 1.000 arquivos renomeados por segundo em segundo plano
durante 5 segundos:

| | |
|---|--:|
| Início até uma pasta listada (100.001 arquivos) | 315 ms |
| A lista acompanha as mudanças | a cada ~750 ms |
| Reler a pasta (thread em segundo plano) | ~70 ms |
| Trabalho na thread da interface por atualização | 7–8 ms |
| Seleção e cursor | ficam no arquivo selecionado |

Tempos de quadro enquanto a mesma pasta é rolada para baixo e para cima por 7.7 s, com a seleção (e portanto o painel de
visualização) mudando a cada meio segundo, cerca de 680 arquivos por segundo renomeados em segundo plano, em uma tela de 200 Hz:

| | mediana | p95 | p99 | pior |
|---|--:|--:|--:|--:|
| Custo de quadro na thread da interface | 2.1 ms | 4.6 ms | 7.9 ms | 16.8 ms |
| Tempo entre quadros | 3.3 ms | 5.3 ms | 9.9 ms | 17.3 ms |

2,182 quadros em 7.7 s; uma pausa maior que um quadro de 60 Hz. Meça você mesmo com
`RUSTYFILES_TRACE_LIVE=1` e `RUST_LOG=warn,rf_app=info`, e registre os quadros com
`GPUI_FRAME_LOG=frames.tsv`.

#### Nova aba em uma janela aberta

| Pasta | Tempo até uma lista estabilizada |
|---|--:|
| 1.000 arquivos | ~48 ms |
| 10.000 arquivos | ~46 ms |
| 100.000 arquivos | ~129 ms |

Cada aba extra custa cerca de 3 MB de memória (1 aba: ~32 MB, 25 abas: ~103 MB).

#### Busca pelo nome do arquivo

O rustyfiles mantém um índice de nomes dos discos locais e responde às buscas a partir dele. Buscando "rapor" em
todas as subpastas de uma pasta de usuário com 3.6 milhões de arquivos e pastas, medido até que os resultados
na tela parassem de mudar:

| | Tempo |
|---|--:|
| rustyfiles 0.4 | **0.38 s** |
| Explorador de Arquivos | 1.8 s (a mais rápida de 4 execuções; as outras levaram 11–32 s) |

O índice guarda cada nome diferente uma única vez: um disco com 4.8 milhões de entradas, mas 490 mil
nomes diferentes, ocupa 61 MB. Um observador de pastas mantém o índice atualizado enquanto o rustyfiles roda; entre execuções,
o diário de alterações do NTFS (lido sem direitos de administrador) informa quais pastas mudaram, então uma
inicialização lista só essas de novo: 0.2 s para o C: aqui, onde uma varredura leva 30 s. Dados de apps, caches,
`node_modules` e `.git` ficam de fora; uma busca iniciada dentro de um deles varre o disco:

| `*.dll` em `C:\Windows` (342,492 itens, 116,851 pastas, cache aquecido) | Tempo |
|---|--:|
| rustyfiles | **7.8 s** |
| `dir /s /b` | 53 s |
| PowerShell `Get-ChildItem -Recurse` | 105 s |

Ele busca em seis threads de prioridade de segundo plano, então o resto da máquina continua responsivo, e
não segue junções, por isso encontra 454 duplicatas a menos.

#### Visualização com Space (DirectLook)

| Arquivo | Conteúdo pronto, desde a tecla pressionada |
|---|--:|
| JPEG, 3840 × 2160 | 24–86 ms |
| Código-fonte, com realce de sintaxe | 23–44 ms |
| Arquivo 7z (conteúdo listado) | 19–31 ms |
| MP3 (tags, letras) | 27–32 ms |
| Vídeo H.264, 1080p | primeiro quadro em 146–185 ms |

#### Arquivos compactados

Lendo RAR com o [rarchive](https://github.com/gelenmedya/rarchive), nosso próprio motor, contra o
UnRAR oficial (decodificar e verificar cada CRC, arquivos no cache do sistema):

| Arquivo | UnRAR | rarchive |
|---|--:|--:|
| 1.3 GB, dicionário de 32 MB, 195 arquivos | 2.79 s | **0.28 s** (10×) |
| 400 MB sólido | 0.95 s | **0.21 s** (4.5×) |
| 400 MB, 520 arquivos | 1.03 s | **0.063 s** (16×) |

#### Copiar e extrair

Nossos próprios motores contra os do Windows (rustyfiles 0.8.6, 2026-11-16):

| | Windows | rustyfiles |
|---|--:|--:|
| Copiar 10.000 arquivos pequenos em 100 pastas (41 MB) | 25.6 s | **1.8 s** (14×) |
| Copiar um arquivo de 1 GB | 0.61 s | 0.44 s |
| Extrair um ZIP de 10.000 arquivos | 32.4 s | **3.6 s** (9×) |

A cópia espalha arquivos pequenos por várias pastas, porque threads gravando numa mesma pasta esperam umas pelas outras; um único arquivo grande é limitado pelo disco, não pelo motor. Ele resolve conflitos de nomes por conta própria
(substituir, pular ou manter os dois), pausa e retoma, pode verificar cada cópia byte a byte e pode limitar
a velocidade. O script de teste de cópia é `crates/rf-win/examples/copybench.rs`.

Todos os números: uma máquina (Ryzen 7 9800X3D, SSD NVMe, Windows 11), novembro de 2026.

</details>

## O que faz

- **Navegação.** Abas que você arrasta para fora, em uma janela, sessões salvas, painel duplo, visualizações Detalhes, grade, lista e colunas, agrupamento, um painel de visualização, marcadores de cor, a Shelf, compartilhamentos de rede e FTP.
- **Arquivos.** Desfazer e refazer, renomeação em lote, tamanhos de pastas sob demanda, uso de disco com treemap, o menu de contexto completo do Explorer, arrastar e soltar com qualquer outro app.
- **Arquivos compactados.** ZIP, 7z, RAR, tar e ISO abrem como pastas; ZIP, 7z e tar podem ser editados por dentro.
- **Busca.** Um índice de nomes dos seus discos, e a busca rápida (<kbd>Alt</kbd>+<kbd>Space</kbd>) para
  apps, arquivos, favoritos e as configurações do Windows.
- **Windows.** Opcionalmente, o gerenciador de arquivos padrão, incluindo <kbd>Win</kbd>+<kbd>E</kbd> e as caixas Abrir e Salvar de outros
  apps.
- **49 idiomas**, seguindo o do sistema.

<details>
<summary><b>Todos os recursos em detalhe</b></summary>

**Navegação.** Abas que você pode arrastar para reordenar ou tirar para uma nova janela. Salve as abas de uma janela,
com os painéis divididos, como uma sessão com nome e abra-a de novo depois. Uma nova aba abre a Página inicial ou a pasta
em que você está (Configurações › Geral). Painel duplo. Visualizações Detalhes, grade, lista e colunas (a lista pode preencher
coluna por coluna, como a do Explorer), itens compactos como a visualização compacta do Explorer, agrupamento, um
painel de visualização, busca, marcadores de cor, a Shelf, bibliotecas, compartilhamentos de rede e FTP. Uma
coluna de status do Git. Tamanhos de pastas sob demanda: clique em "Calcular tamanho"; eles são lembrados entre reinícios e podem
mostrar o tamanho em disco. "Mostrar uso do disco" em uma pasta ou unidade lista o que ocupa o espaço, do maior para o menor,
ao lado de um treemap em que você pode clicar. Pastas virtuais como Bibliotecas e Galeria abrem no
rustyfiles, e os dispositivos que o Windows lista em Este computador (celulares por USB ou Phone Link) aparecem em
Dispositivos na Página inicial. As pastas de nuvem que o Windows conhece (OneDrive, Proton Drive, Dropbox, MEGA e
outras) ficam na barra lateral.

**Arquivos.** Desfazer e refazer para renomeações, cópias, movimentos, exclusão para a Lixeira e achatamento de pastas.
Renomeação em lote. Duplicar, copiar ou mover para o outro painel, e colar texto copiado ou uma
imagem como um arquivo novo. O menu de contexto completo do Explorer com Enviar para, arrastar e soltar com o Explorer, a
área de trabalho, navegadores e Outlook, e destinos de soltura nas abas, pastas fixadas, pastas e programas. Envie
arquivos para os seus outros computadores com o [Ferry](#ferry), e baixe com o [HyperDownload](#hyperdownload).

**Arquivos compactados.** ZIP, 7z, RAR, a família tar e ISO abrem como pastas, incluindo ZIP, 7z
e RAR protegidos por senha. Adicione, exclua e renomeie dentro de ZIP, 7z e tar. Crie ZIP, 7z e tar.zst em todos os
núcleos. A leitura é feita pelo [rarchive](https://github.com/gelenmedya/rarchive), nosso próprio
motor em Rust puro (também [no crates.io](https://crates.io/crates/rarchive)): ele decodifica um arquivo WinRAR de 1.3 GB
dez vezes mais rápido que o UnRAR.

**Windows.** Opcionalmente o gerenciador de arquivos padrão: pastas, <kbd>Win</kbd>+<kbd>E</kbd>, Este computador
e a Lixeira abrem no rustyfiles. Opcionalmente, as caixas Abrir, Salvar e Selecionar pasta de outros
apps também abrem no rustyfiles (Configurações › Avançado), e um botão de fixar ali mantém uma pasta inicial para cada
app. Imagens e vídeos abertos no Fotos ou em outro app da Store vêm com a pasta deles, para que o
Anterior e o Próximo funcionem. Os caminhos `/select,` e `shell:` do Explorer funcionam. Mica, modo escuro e a
cor de destaque do sistema. Quando um jogo ou uma renderização satura a GPU, o desenho passa para a CPU e volta
sem piscar.

**Comandos na barra de endereço.** Como no Explorer: pressione <kbd>Ctrl</kbd>+<kbd>L</kbd>, digite `cmd`,
`powershell`, `wt` ou `code .` e pressione <kbd>Enter</kbd>. O texto é primeiro tomado como um lugar (um
caminho, uma unidade, `%VAR%`, `shell:`); uma pasta ou arquivo com esse nome na pasta aberta sempre vence.
Só quando não há tal lugar é que a primeira palavra é procurada como programa, do jeito que a caixa Executar faz,
e iniciada na pasta aberta. Nada é iniciado a partir de uma sugestão, e nenhum direito de administrador é
pedido. Uma chave em Configurações › Arquivos e pastas › Comportamentos desliga isso.

**Organização.** Regras para uma pasta: condições sobre nome, extensão, tipo, tamanho e idade, e uma ação
(mover, mover para uma pasta montada a partir de um modelo, renomear ou a Lixeira). Uma execução manual mostra
cada passo em uma prévia e não move nada antes de Aplicar, e uma execução pode ser desfeita. Nada é excluído
para sempre e nada é sobrescrito. Regras que rodam sozinhas vêm desligadas por padrão.

**Busca.** A busca rápida (<kbd>Alt</kbd>+<kbd>Space</kbd>) encontra apps, arquivos e pastas, favoritos do
navegador e as páginas de configurações do Windows. O que você abre com frequência vem primeiro, e o que você escolheu para as
mesmas letras antes é a primeira linha.

**Idiomas.** São 49, seguindo o idioma de exibição do sistema (ou escolha um em Configurações ›
Geral). A interface é escrita em turco e inglês; os outros idiomas partiram do texto da comunidade
Files e são traduzidos por máquina a partir dele. Nomes e textos de direita para a esquerda leem da direita para a
esquerda (graças a @ialhabbal); a interface em si ainda não é espelhada. Correções são bem-vindas:
cada idioma é um arquivo JSON em `crates/rf-app/locales/`.

</details>

<img alt="rustyfiles mostrando uma pasta de papéis de parede do Windows em visualização de grade, uma delas selecionada e exibida no painel de visualização à direita, sobre o fundo HyperGlass" src="docs/readme/grid.png" width="100%">

## DirectLook

Selecione um arquivo e pressione <kbd>Space</kbd>: vídeo e áudio com o próprio player, imagens, texto e
código, o conteúdo de arquivos compactados, e um cartão de detalhes para todo o resto. As teclas de seta percorrem a
pasta enquanto ela permanece aberta. Ele roda como um programa próprio, então um vídeo pesado nunca pesa no gerenciador de arquivos, e se você ligar a opção ele funciona no Explorer e na área de trabalho também (Configurações › DirectLook).

## Ferry

Envie arquivos e pastas para o rustyfiles nos seus outros computadores na mesma rede, do jeito que o AirDrop
faz: clique com o botão direito › **Enviar com Ferry**. Desligado até você ligá-lo em Configurações › Ferry.

<img alt="Configurações › Ferry com dois dispositivos próximos, um PC Windows pareado com a velocidade medida e um MacBook com uma estimada; uma notificação do Windows pergunta se deve aceitar arquivos do MacBook, e a barra de status mostra uma transferência em 61 por cento" src="docs/readme/ferry.png" width="100%">

<sub>Um desenho de Configurações › Ferry com dispositivos inventados, nas palavras e no layout do próprio app.</sub>

<details>
<summary><b>Como o Ferry funciona</b></summary>

- Criptografado de ponta a ponta (QUIC com TLS 1.3), direto de dispositivo para dispositivo: sem servidor, sem conta,
  nada sai da sua rede.
- Pareie os seus dispositivos do jeito que você pareia o Bluetooth: os dois mostram o mesmo código de seis dígitos. Os arquivos de um dispositivo pareado
  podem chegar sem perguntar; qualquer outro é perguntado antes, em uma notificação.
- Você escolhe quem pode ver o seu computador: ninguém, os seus dispositivos pareados (o padrão) ou todos por
  dez minutos. Enquanto Configurações › Ferry está aberto, todos podem vê-lo, para que um dispositivo novo o encontre.
- Uma transferência interrompida continua de onde parou, e cada arquivo é verificado antes de receber o seu
  nome. Os arquivos recebidos são marcados como baixados, então o SmartScreen pergunta antes que um programa entre eles
  rode.
- Cada dispositivo mostra uma velocidade estimada, e **Testar velocidade** mede nos dois sentidos em poucos segundos.
- O app do macOS fala o mesmo protocolo, descrito em [docs/nearby-protocol.md](docs/nearby-protocol.md).
  O Firewall do Windows pode perguntar sobre o Ferry: permita para redes privadas.

</details>

## HyperDownload

O gerenciador de downloads próprio do rustyfiles, um complemento: Configurações › Complementos › HyperDownload › **Instalar**. Um
arquivo por muitas conexões ao mesmo tempo, pausado e retomado de onde parou, torrents e links magnet,
uma página de Downloads, e vídeos que tocam no DirectLook antes de terminarem. É um programa separado
([código-fonte](https://github.com/gelenmedya/hyperdownload), MIT), baixado só quando você clica em
Instalar, verificado pelo seu SHA-256 antes, e removível a qualquer momento.

## Experimental no Windows: HyperGlass, o Dock e a área de trabalho

Desligado por padrão e no começo: construído e testado em um PC com dois monitores. Espere arestas ásperas, e por favor
relate o que encontrar. O Explorer continua rodando por baixo; nada do Windows é removido ou substituído no
disco.

<img alt="Uma área de trabalho com o Dock do rustyfiles na parte de baixo (três ilhas: Iniciar e busca, os apps fixados e abertos, a bandeja e o relógio), o painel Iniciar do Dock aberto à esquerda com apps fixados e arquivos recentes, o HyperSearch no meio com os apps encontrados para adobe, e a central de controle à direita com Wi-Fi, Bluetooth, Ethernet, a música tocando, volume e brilho" src="docs/readme/shell.png" width="100%">

<sub>Iniciar, HyperSearch e a central de controle, montados a partir de três capturas da mesma área de trabalho (elas não abrem todas ao mesmo tempo).</sub>

<details>
<summary><b>O que cada parte faz</b></summary>

- **HyperGlass** (Configurações › Aparência) é um quinto fundo: o fundo da janela é vidro que dobra e
  embaça o que está atrás dele.
- **O Dock** (Configurações › Dock, um interruptor) põe três ilhas de vidro na parte de baixo de cada monitor
  no lugar da barra de tarefas do Windows: um painel Iniciar e busca próprios, os apps abertos e fixados com
  prévias das janelas, os ícones da bandeja, uma central de controle e um painel de notificações. Enquanto está ligado, um toque
  na tecla do Windows abre o Iniciar do Dock, e <kbd>Alt</kbd>+<kbd>Tab</kbd> abre o seu alternador.
  Desligue o Dock e os do Windows voltam.
- **A área de trabalho** desenhada pelo rustyfiles vem com o Dock: o seu papel de parede e os ícones, com uma opção
  para cantos arredondados.
- **Brilho para cada monitor.** O Windows tem um controle deslizante só para a tela de um notebook; a
  central de controle do Dock tem um para cada monitor. Monitores externos são ajustados por DDC/CI, que quase todo
  monitor de mesa dos últimos quinze anos fala por DisplayPort, HDMI e DVI. Se um monitor não tiver
  controle deslizante, verifique se o DDC/CI está ligado no menu dele; alguns docks, switches KVM e adaptadores não
  repassam isso.

</details>

## macOS (beta)

O mesmo gerenciador de arquivos, desenhado nativamente com AppKit e Liquid Glass sobre o mesmo núcleo em Rust: abas,
painel duplo, todas as visualizações, Quick Look com <kbd>Space</kbd>, arquivos compactados como pastas, renomeação em lote, uso do disco,
a Shelf, marcadores de cor, status do Git, Conectar ao Servidor, Ferry e os mesmos 49 idiomas. O Dock, a
área de trabalho, o HyperSearch e o DirectLook continuam no Windows por enquanto, e a versão do HyperDownload para Mac vem
na sua própria versão. Apple silicon e Intel, com macOS 26 (Tahoe). Ele se atualiza sozinho como o app do Windows.

## Instalação

**Windows.** [Baixe rustyfiles-win-Setup.exe](https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe)
(cerca de 50 MB) e abra-o. Se o Windows disser "O Windows protegeu o seu PC", clique em **Mais informações**, depois
em **Executar assim mesmo**: o instalador ainda não tem assinatura de código, e cada versão é verificada no
[VirusTotal](https://www.virustotal.com) (os relatórios estão linkados no fim das notas da versão).
O rustyfiles abre sozinho e fica no seu menu Iniciar. Sem senha de administrador; as atualizações são instaladas
silenciosamente em segundo plano.

**macOS (beta).** [Baixe rustyfiles-mac.dmg](https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-mac.dmg),
abra-o e arraste o rustyfiles para Aplicativos. Ele é assinado e notarizado pela Apple, então abre como qualquer outro
app do Mac.

<details>
<summary><b>Scoop, portátil, ARM64, gerenciador de arquivos padrão, desinstalação</b></summary>

**Com o [Scoop](https://scoop.sh):**

```powershell
scoop bucket add rustyfiles https://github.com/gelenmedya/scoop-rustyfiles
scoop install rustyfiles/rustyfiles
```

**Portátil ou ARM64:** um zip portátil e um instalador ARM64 (`rustyfiles-win-arm64-Setup.exe`) estão na
página de [Releases](https://github.com/gelenmedya/rustyfiles/releases). É necessário o Windows 11
(o Windows 10 não foi testado). Linux está planejado.

**Pastas e <kbd>Win</kbd>+<kbd>E</kbd> no rustyfiles:** ligue em Configurações › Avançado › Definir como
gerenciador de arquivos padrão. Você pode desligar a qualquer momento.

**Desinstale** como qualquer app, em Configurações do Windows › Aplicativos. Tudo o que o rustyfiles alterou volta para
o Explorador de Arquivos.

**Caixas Abrir/Salvar de outros apps falham ou travam** depois que os arquivos do rustyfiles foram removidos sem uma
desinstalação (um antivírus, uma atualização quebrada)? Abrir o rustyfiles de novo resolve. Se ele não
abrir mais, este comando do PowerShell devolve a caixa de diálogo do próprio Windows a todos os apps:

```powershell
Remove-Item -Recurse "HKCU:\Software\Classes\CLSID\{DC1C5A9C-E88A-4dde-A5A1-60F82A20AEF7}", "HKCU:\Software\Classes\CLSID\{C0B4E2F3-BA21-4773-8DBA-335EC946EB8B}"
```

</details>

<details>
<summary><b>Compilar a partir do código-fonte</b></summary>

Você precisa de [Rust](https://rustup.rs) 1.95 ou mais novo e do Visual Studio Build Tools com a
carga de trabalho "Desenvolvimento para desktop com C++" (uma dependência, zstd, compila código C).

```powershell
git clone https://github.com/gelenmedya/rustyfiles
cd rustyfiles
.\scripts\install.ps1            # builds, then installs for your user
.\scripts\install.ps1 -Default   # also makes it the default file manager
.\scripts\install.ps1 -Uninstall
```

`cargo run --release -p rf-app` inicia o app sem instalar. O app do macOS fica em `macos/` e
`crates/rf-mac`, fora do workspace do Windows.

</details>

**Dúvidas ou ideias?** Venha dizer oi no [Discord](https://discord.gg/K2banZ36zJ), ou visite
[rustyfiles.com](https://rustyfiles.com).

## Por dentro

<details>
<summary><b>Teclado</b></summary>

| Teclas | Ação |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>T</kbd> / <kbd>Ctrl</kbd>+<kbd>W</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd> | Nova aba, fechar aba, reabrir aba fechada |
| <kbd>Ctrl</kbd>+<kbd>L</kbd> ou <kbd>Alt</kbd>+<kbd>D</kbd> | Barra de endereço |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> | Buscar (<kbd>Enter</kbd> busca também nas subpastas) |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> ou <kbd>F1</kbd> | Paleta de comandos |
| <kbd>Ctrl</kbd>+<kbd>P</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>A</kbd> | Encontrar uma pasta visitada, encontrar uma aba |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>1</kbd> … <kbd>5</kbd> | Detalhes, grade, colunas, cartões, lista |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd>, <kbd>F6</kbd> | Painel duplo, trocar de painel |
| <kbd>Shift</kbd>+<kbd>F5</kbd> / <kbd>Shift</kbd>+<kbd>F6</kbd> | Copiar, mover para o outro painel |
| <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>D</kbd> | Duplicar |
| <kbd>Alt</kbd>+<kbd>P</kbd>, <kbd>Ctrl</kbd>+<kbd>B</kbd> | Painel de visualização, barra lateral |
| <kbd>Space</kbd> | DirectLook |
| <kbd>F2</kbd> | Renomear (renomeação em lote com vários itens selecionados) |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> / <kbd>Ctrl</kbd>+<kbd>Y</kbd> | Desfazer, refazer |
| <kbd>Ctrl</kbd>+<kbd>H</kbd> | Mostrar itens ocultos |

Todo atalho pode ser alterado em Configurações › Ações: clique em um comando e depois pressione as novas teclas.

</details>

<details>
<summary><b>Estrutura do projeto</b></summary>

| Crate | O que contém |
|---|---|
| `crates/rf-app` | A interface do Windows, e os seus dois programas: `rustyfiles` e `directlook` (`src/look/`) |
| `crates/rf-win` | Integração com o Windows e o shell, o motor de operações de arquivos |
| `crates/rf-core` | O modelo compartilhado pelas duas plataformas: locais, histórico, entradas, formatos |
| `crates/rf-archive` | Operações de arquivos compactados: navegar, extrair, criar, editar |
| `crates/rf-nearby` | Ferry, compartilhado com o app do macOS |
| `crates/rf-player` | O player de vídeo e áudio do DirectLook |
| `crates/rustdecoder` | Nosso próprio motor de vídeo, em andamento: MP4/MOV e MKV/WebM; H.264, HEVC, VP9 e AV1 decodificados na placa de vídeo por D3D11 Video, bit a bit iguais ao ffmpeg / dav1d nos seus testes; ProRes e DNxHR na CPU; áudio e sincronia A/V ([design](docs/design/rustdecoder.md)) |
| `macos/`, `crates/rf-mac` | O app do macOS (AppKit) e a sua ponte para o núcleo em Rust |

A interface do Windows é construída com [GPUI](https://gpui.rs), o framework por trás do editor Zed. A sua
camada do Windows é um fork que adiciona a troca GPU/CPU:
[gelenmedya/gpui-pre-windows](https://github.com/gelenmedya/gpui-pre-windows).

</details>

<details>
<summary><b>Variáveis de ambiente</b></summary>

| Variável | Efeito |
|---|---|
| `RUSTYFILES_RENDERER` = `gpu`, `cpu` ou `auto` | Força onde a janela é desenhada |
| `RUSTYFILES_CONFIG_DIR` = pasta | Guarda as configurações em outra pasta (uso portátil) |
| `RUSTYFILES_MULTI_INSTANCE=1` | Permite várias cópias ao mesmo tempo (testes) |
| `RUSTYFILES_FRAME_LOG=1` | Registra os tempos de quadro |
| `RUSTYFILES_LOOK_TRACE=1` | Registra os tempos do DirectLook no stderr |
| `RUSTYFILES_MUTE=1` | O DirectLook toca sem som (testes) |

As configurações ficam em `%APPDATA%\rustyfiles\settings.toml`, e as do DirectLook em `directlook.toml`, ao lado.

</details>

<details>
<summary><b>Como é feito</b></summary>

O rustyfiles é escrito com ferramentas de programação com IA, e isso não é segredo. Um desenvolvedor decide o que é
construído e como deve se comportar, testa cada mudança em um PC de verdade (dois monitores, trabalho do dia a dia) e
descarta o que não se sustenta. A IA escreve boa parte do código; quem decide o que é lançado é uma pessoa.

O que mantém isso honesto:

- **Testes.** Mais de 4.000 testes automatizados rodam antes de cada mudança ser enviada.
- **Medições.** Os números de velocidade vêm de um teste integrado ao app (Configurações › Teste de velocidade),
  então qualquer pessoa pode conferi-los na própria máquina.
- **Relatos.** Os bugs vêm de quem usa. Cada issue recebe uma resposta, a correção é feita às claras, e quem
  relatou é convidado a testá-la antes de ela ser fechada.
- **O código.** Todo ele está aqui sob a licença MIT, para ler e julgar.

Por que Rust: um gerenciador de arquivos precisa continuar rápido com 100.000 arquivos em uma pasta e nunca pode perder os
dados de ninguém. Rust dá velocidade sem coletor de lixo e elimina classes inteiras de travamentos antes de
serem lançadas. O rustyfiles é um app novo escrito do zero, não uma reescrita do Explorer; ele roda ao lado do
Explorer e não altera nada do Windows no disco.

</details>

## Status

O rustyfiles é novo, e o seu autor o usa todo dia no lugar do Explorer. O app do macOS é uma primeira
beta sobre o mesmo núcleo, e o Linux vem depois dele. Relatos de bugs e pull requests são bem-vindos.

## Listado em

- [AlternativeTo](https://alternativeto.net/software/rustyfiles/about/), como alternativa ao Explorador de Arquivos
- [Awesome GPUI](https://github.com/zed-industries/awesome-gpui#productivity), na seção Productivity
- [Awesome Rust](https://github.com/rust-unofficial/awesome-rust#file-manager), na seção de gerenciadores de arquivos
- [Scoop](https://scoop.sh), pelo seu próprio bucket (veja [Instalação](#instalação))

## Agradecimentos

O rustyfiles se apoia no trabalho de outras pessoas, e pegou emprestadas muitas ideias pelo caminho.

[Files](https://github.com/files-community/Files) mostrou como pode ser um gerenciador de arquivos moderno para Windows.
O conjunto de recursos dele foi a nossa lista de verificação, as configurações e o layout dele moldaram os nossos, e a
abordagem dele para substituir o Explorer como gerenciador de arquivos padrão guiou a nossa. Obrigado à comunidade do Files.

[GPUI](https://gpui.rs), o framework de interface do editor [Zed](https://github.com/zed-industries/zed),
desenha cada pixel do rustyfiles no Windows na GPU e torna possível uma janela tão rápida. Obrigado à
equipe do Zed por construí-lo e compartilhá-lo, e à [Longbridge](https://github.com/longbridge/gpui-kit)
pelo gpui-kit, os componentes em que nos apoiamos.

O DirectLook segue a ideia do Quick Look do macOS, e o [QuickLook](https://github.com/QL-Win/QuickLook)
de Paddy Xu, que o levou ao Windows primeiro. O rarchive aprendeu o formato RAR a partir do código-fonte do UnRAR
de Alexander Roshal.

Construído com [windows-rs](https://github.com/microsoft/windows-rs), [Velopack](https://velopack.io)
(instalação e atualizações), ícones [Lucide](https://lucide.dev), [zip](https://github.com/zip-rs/zip2),
[sevenz-rust2](https://github.com/hasenbanck/sevenz-rust), [tar](https://github.com/alexcrichton/tar-rs),
[notify](https://github.com/notify-rs/notify) e muitos outros crates. As imagens usam
[Bricolage Grotesque](https://github.com/ateliertriay/bricolage) de Atelier Triay,
[Instrument Sans](https://fonts.google.com/specimen/Instrument+Sans) e
[Doto](https://fonts.google.com/specimen/Doto).

## Licença

MIT, veja [LICENSE](LICENSE). O fork do GPUI mantém a licença Apache-2.0 do Zed no próprio repositório.
