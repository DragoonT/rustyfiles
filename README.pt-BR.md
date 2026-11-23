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

<img alt="rustyfiles: um gerenciador de arquivos para Windows que nunca trava. Escrito em Rust. Desenhado como uma janela do rustyfiles numa pasta de 100,000 arquivos, aberta em 281 ms" src="docs/readme/cover.png" width="100%">

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest"><img alt="Última versão" src="https://img.shields.io/github/v/release/gelenmedya/rustyfiles?label=download&labelColor=232326&color=ff6a1a"></a>
  <img alt="Windows 11, x64 e ARM64" src="https://img.shields.io/badge/Windows_11-x64_%7C_ARM64-3d3d44?labelColor=232326">
  <a href="LICENSE"><img alt="Licença MIT" src="https://img.shields.io/badge/license-MIT-3d3d44?labelColor=232326"></a>
</p>

rustyfiles é um gerenciador de arquivos para Windows, escrito do zero em Rust. Ele foi feito para
substituir o Explorador de Arquivos no uso do dia a dia: abas, painel duplo, arquivos compactados
que você abre como pastas e um visualizador no estilo Quick Look chamado DirectLook.

A janela nunca espera pelo disco. Listagem, ícones, miniaturas, o menu de contexto e cada operação
de arquivo rodam fora da thread da interface, então um compartilhamento de rede lento ou uma pasta
com 100,000 arquivos não consegue travá-la.

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe"><img alt="Baixar para Windows" src="https://img.shields.io/badge/Download_for_Windows-FF6A1A?style=for-the-badge&logo=windows11&logoColor=white"></a>
  <a href="https://rustyfiles.com"><img alt="Site" src="https://img.shields.io/badge/rustyfiles.com-232326?style=for-the-badge&logo=googlechrome&logoColor=white"></a>
  <a href="https://discord.gg/K2banZ36zJ"><img alt="Entre no Discord" src="https://img.shields.io/badge/Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white"></a>
</p>

<p align="center">
  <a href="https://www.buymeacoffee.com/ozerguzel"><img alt="Me pague um café" src="https://cdn.buymeacoffee.com/buttons/v2/default-yellow.png" height="44"></a>
</p>

<p align="center">
  <a href="https://alternativeto.net/software/rustyfiles/about/?utm_source=badge&utm_medium=referral">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://alternativeto.net/static/badges/badge-compact-dark.svg">
      <img alt="rustyfiles no AlternativeTo" src="https://alternativeto.net/static/badges/badge-compact-light.svg" height="54">
    </picture>
  </a>
</p>

Gratuito, sem conta, sem anúncios. Baixe, dê dois cliques, pronto: veja [Instalação](#instalação).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/readme/projects-dark.png">
  <img alt="rustyfiles mostrando uma pasta Projects em modo de detalhes, com abas, uma barra de caminho e pastas fixadas na barra lateral; a janela é vidro sobre o papel de parede da área de trabalho" src="docs/readme/projects-light.png" width="100%">
</picture>

<sub>As imagens mostram o fundo experimental HyperGlass (Configurações › Aparência). O fundo padrão é o Mica, como no Explorador de Arquivos.</sub>

## Velocidade

<img alt="Um teste de velocidade desenhado como uma lista do rustyfiles: abrir uma pasta com 100,000 arquivos leva 281 ms no rustyfiles e 2,318 ms no Explorador de Arquivos (8 vezes mais rápido); uma pasta com 2,000 imagens, 278 ms contra 2,177 ms (8 vezes); copiar 10,000 arquivos pequenos, 1.8 s contra 25.6 s (14 vezes); extrair um ZIP de 10,000 arquivos, 3.6 s contra 32.4 s (9 vezes)" src="docs/readme/speed.png" width="100%">

### Abrindo uma pasta

Tempo desde o início do app até a lista da pasta ficar parada, em uma janela nova, com a pasta já
no cache de arquivos do sistema. Mediana de três execuções depois de um aquecimento, medida pelo
teste de velocidade embutido no rustyfiles (Configurações › Teste de velocidade): qualquer pessoa
pode executá-lo na própria máquina.

| Pasta | rustyfiles | Explorador de Arquivos | [Files](https://files.community) |
|---|--:|--:|--:|
| Pasta vazia | 281 ms | 1,006 ms | 813 ms |
| 1,000 arquivos | 284 ms | 2,214 ms | 1,893 ms |
| 10,000 arquivos | 286 ms | 2,221 ms | 1,053 ms |
| 100,000 arquivos | 281 ms | 2,318 ms | 4,069 ms |
| 2,000 imagens (miniaturas) | 278 ms | 2,177 ms | 1,075 ms |
| Memória após listar 10,000 arquivos | 39 MB | 69 MB | 204 MB |

Medido em uma máquina (Ryzen 7 9800X3D, SSD NVMe, Windows 11) em 2026-11-16, com o rustyfiles
0.8.6 e o Files 4.3 (a 0.8.5 levou cerca de 540 ms no mesmo teste: a primeira aba dela tocava uma
animação de abertura que a 0.8.6 não toca mais na inicialização). O rustyfiles e o Explorador
iniciaram cada um um processo novo. O Files já estava rodando em segundo plano, então os tempos
dele não são de uma inicialização a frio; ele também preenche a lista em etapas, e o tempo conta
até a imagem parar de mudar. Memória é o working set privado, o número que o Gerenciador de Tarefas
mostra; para o Files é o do processo em execução.

Com "manter em execução em segundo plano" ativado, uma nova janela do rustyfiles não inicia um
processo: em uma medição anterior (2026-11-06, uma versão anterior) ela mostrou uma pasta listada
em 93–168 ms.

### Uma pasta com 100,000 arquivos que não para de mudar

A pasta aberta, um arquivo selecionado, e cerca de 1,000 arquivos renomeados por segundo em
segundo plano durante 5 segundos:

| | |
|---|--:|
| Abertura até uma pasta listada (100,001 arquivos) | 315 ms |
| A lista acompanha as mudanças | a cada ~750 ms |
| Releitura da pasta (thread em segundo plano) | ~70 ms |
| Trabalho na thread da interface por atualização | 7–8 ms |
| Seleção e cursor | permanecem no arquivo selecionado |

A janela nunca espera pela pasta: a releitura acontece fora da thread da interface, e a atualização
em si custa menos que um quadro a 120 Hz. Meça você mesmo com `RUSTYFILES_TRACE_LIVE=1` e
`RUST_LOG=warn,rf_app=info`.

Tempos de quadro enquanto isso acontece: a mesma pasta rolada para baixo e de volta para cima com a
roda do mouse por 7.7 s, com a seleção (e assim o painel de pré-visualização) se movendo a cada meio
segundo, enquanto cerca de 680 arquivos por segundo são renomeados em segundo plano. Tela de 200 Hz.

| | mediana | p95 | p99 | pior |
|---|--:|--:|--:|--:|
| Custo do quadro na thread da interface | 2.1 ms | 4.6 ms | 7.9 ms | 16.8 ms |
| Tempo entre quadros | 3.3 ms | 5.3 ms | 9.9 ms | 17.3 ms |

2,182 quadros em 7.7 s; uma pausa mais longa que um quadro de 60 Hz (16.7 ms). Registre o seu com
`GPUI_FRAME_LOG=frames.tsv` (uma linha por quadro: seu início e seu custo, em ms).

### Nova aba em uma janela aberta

| Pasta | Tempo até uma lista estabilizada |
|---|--:|
| 1,000 arquivos | ~48 ms |
| 10,000 arquivos | ~46 ms |
| 100,000 arquivos | ~129 ms |

Cada aba extra custa cerca de 3 MB de memória (1 aba: ~32 MB, 25 abas: ~103 MB).

### Busca por nome de arquivo

O rustyfiles mantém um índice de nomes dos drives locais e responde às buscas a partir dele.
Busca em todas as subpastas de uma pasta de usuário com 3.6 milhões de arquivos e pastas por
"rapor", cronometrada até os resultados na tela pararem de mudar:

| | Tempo |
|---|--:|
| rustyfiles 0.4 | **0.38 s** |
| Explorador de Arquivos | 1.8 s (o mais rápido de 4 execuções; as outras levaram 11–32 s) |

O índice guarda cada nome diferente uma única vez: um drive com 4.8 milhões de entradas, mas 490 mil
nomes diferentes, ocupa 61 MB. Um observador de pastas mantém o índice atualizado enquanto o
rustyfiles roda. Entre as execuções, o diário de alterações do NTFS (lido sem direitos de
administrador) informa quais pastas mudaram, então uma inicialização lista só essas de novo em vez
de percorrer os drives: 0.2 s para o C: aqui, onde uma varredura leva 30 s. Dados de aplicativos,
caches, pastas `node_modules` e `.git` ficam fora do índice; uma busca iniciada dentro de uma delas
percorre o disco:

`*.dll` em `C:\Windows`: 342,492 itens em 116,851 pastas, com o cache de arquivos aquecido.

| Método | Tempo |
|---|--:|
| rustyfiles | **7.8 s** |
| `dir /s /b` | 53 s |
| PowerShell `Get-ChildItem -Recurse` | 105 s |

O rustyfiles pesquisa em seis threads de prioridade baixa, então o resto da máquina continua
responsivo. Ele não segue junções, por isso encontra 454 duplicatas a menos.

### Pré-visualização com Espaço (DirectLook)

Tempo entre a tecla pressionada e o conteúdo na tela, com o app já aberto:

| Arquivo | Conteúdo pronto |
|---|--:|
| JPEG, 3840 × 2160 | 24–86 ms |
| Código-fonte, com realce de sintaxe | 23–44 ms |
| Arquivo 7z (conteúdo listado) | 19–31 ms |
| MP3 (tags, letras) | 27–32 ms |
| Vídeo H.264, 1080p | primeiro quadro em 146–185 ms |

### Arquivos compactados

Leitura de RAR com [rarchive](https://github.com/gelenmedya/rarchive), nosso próprio motor, comparada
com o UnRAR oficial (decodificando e verificando cada CRC, arquivos no cache do sistema):

| Arquivo | UnRAR | rarchive |
|---|--:|--:|
| 1.3 GB, dicionário de 32 MB, 195 arquivos | 2.79 s | **0.28 s** (10×) |
| 400 MB sólido | 0.95 s | **0.21 s** (4.5×) |
| 400 MB, 520 arquivos | 1.03 s | **0.063 s** (16×) |

### Copiar e extrair

O mesmo teste de velocidade, nossos motores contra os do Windows (rustyfiles 0.8.6, 2026-11-16):

| | Windows | rustyfiles |
|---|--:|--:|
| Copiar 10,000 arquivos pequenos em 100 pastas (41 MB) | 25.6 s | **1.8 s** (14×) |
| Copiar um arquivo de 1 GB | 0.61 s | 0.44 s |
| Extrair um ZIP de 10,000 arquivos | 32.4 s | **3.6 s** (9×) |

A cópia usa um motor próprio e paralelo, que espalha os arquivos pequenos por várias pastas, porque
threads gravando em uma mesma pasta ficam esperando umas pelas outras. Um arquivo grande único é
limitado pelo disco, não pelo motor (em uma execução anterior o Windows foi o mais rápido dos dois,
por alguns ms). Buscar por nome nas pastas de teste (123,000 entradas) levou 55 ms.

Ele também resolve conflitos de nome sozinho (substituir, pular ou manter os dois, para todos ou
por arquivo), pausa e retoma, pode verificar cada cópia byte a byte e pode limitar a velocidade.

Todos os números: uma máquina (Ryzen 7 9800X3D, SSD NVMe, Windows 11), novembro de 2026. Abertura de
pasta, cópia e extração vêm do teste de velocidade embutido (Configurações › Teste de velocidade);
os outros scripts de teste estão no repositório (`crates/rf-win/examples/copybench.rs` para cópia).

## O que ele faz

**Navegação.** Abas que você pode arrastar para reordenar ou soltar em uma nova janela. Salve as
abas de uma janela, incluindo os painéis divididos, como uma sessão com nome e reabra depois; o nome
fica ao lado das abas. Uma nova aba abre a Página inicial ou a pasta em que você está (Configurações ›
Geral). Painel duplo. Visualizações de detalhes, grade, lista e colunas (a lista pode preencher coluna
por coluna, como faz o Explorador), agrupamento, painel de pré-visualização, busca, etiquetas de cor,
a Shelf, bibliotecas, compartilhamentos de rede e FTP. Uma coluna de status do Git. Tamanhos de
pasta sob demanda: clique em "Calcular tamanho"; eles são lembrados entre reinicializações e podem
mostrar o tamanho em disco. "Mostrar uso do disco" em uma pasta ou drive lista o que ocupa espaço,
do maior para o menor, ao lado de um treemap em que você pode clicar (C:\Windows, 341 mil entradas:
26 MB em memória). Pastas virtuais como Bibliotecas e Galeria abrem no rustyfiles, e dispositivos que
o Windows lista em Este Computador (celulares via USB ou Phone Link) aparecem em Dispositivos na
página inicial; essa parte ainda não foi testada com um celular real.

**Arquivos.** Desfazer e refazer para renomear, copiar, mover, excluir para a Lixeira e achatar
pastas. Renomeação em massa. Duplicar, copiar ou mover para o outro painel, e colar texto copiado
ou uma imagem como um novo arquivo. O menu de contexto completo do Explorador de Arquivos, com
Enviar para, arrastar e soltar com o Explorador, a área de trabalho, navegadores e o Outlook, e
destinos para soltar em abas, pastas fixadas, pastas e programas. Envie arquivos para seus outros
computadores com [Ferry](#ferry), e baixe com [HyperDownload](#hyperdownload).

**Arquivos compactados.** ZIP, 7z, RAR, a família tar e ISO abrem como pastas, incluindo ZIP, 7z e
RAR protegidos por senha. Adicionar, excluir e renomear dentro de ZIP, 7z e tar. Criar ZIP, 7z e
tar.zst usando todos os núcleos do processador. A leitura é feita pelo
[rarchive](https://github.com/gelenmedya/rarchive), nosso próprio motor em Rust puro (também
[no crates.io](https://crates.io/crates/rarchive) para outros projetos em Rust): ele decodifica um
arquivo WinRAR de 1.3 GB dez vezes mais rápido que o UnRAR.

**Windows.** Opcionalmente, o gerenciador de arquivos padrão: pastas, <kbd>Win</kbd>+<kbd>E</kbd>,
Este Computador e a Lixeira abrem no rustyfiles. Opcionalmente, as caixas Abrir, Salvar e Selecionar
pasta de outros apps também abrem no rustyfiles (Configurações › Avançado), e um botão de fixar ali
mantém uma pasta inicial para cada app. Imagens e vídeos abertos no Fotos ou em outro app da Microsoft
Store vêm com a sua pasta, para que Anterior e Próximo funcionem. Os caminhos `/select,` e `shell:`
do Explorador funcionam. Mica, modo escuro e a cor de destaque do sistema. Quando um jogo ou uma
renderização sobrecarrega a GPU, o desenho passa para a CPU e volta sem piscar.

**Comandos na barra de endereço.** Como no Explorador: pressione <kbd>Ctrl</kbd>+<kbd>L</kbd>, digite
`cmd`, `powershell`, `wt` ou `code .` e pressione <kbd>Enter</kbd>. O texto é primeiro interpretado
como um lugar (um caminho, um drive, `%VAR%`, `shell:`); uma pasta ou arquivo com esse nome na pasta
aberta sempre vence. Só quando não existe esse lugar é que a primeira palavra é procurada como
programa, do mesmo jeito que a caixa Executar faz, e iniciada na pasta aberta com o resto como
argumentos. Nada é iniciado a partir de uma sugestão de preenchimento, e nenhum direito de
administrador é pedido. Uma chave em Configurações › Arquivos e pastas › Comportamentos desliga isso.

**Organização.** Regras para uma pasta: condições de nome, extensão, tipo, tamanho e idade, e uma
ação (mover, mover para uma pasta criada a partir de um modelo, renomear ou enviar para a Lixeira).
Uma execução manual mostra cada passo em uma prévia e não move nada antes de Aplicar, e uma execução
pode ser desfeita. Nada é excluído definitivamente e nada é sobrescrito. Regras que rodam sozinhas
vêm desligadas por padrão.

**Busca.** A busca rápida (<kbd>Alt</kbd>+<kbd>Espaço</kbd>) encontra apps, arquivos e pastas,
favoritos do navegador e páginas de configurações do Windows. O que você abre com frequência aparece
primeiro, e o que você escolheu antes para as mesmas letras é a primeira linha.

**Idiomas.** São 49, seguindo o idioma de exibição do Windows (ou escolha um em Configurações ›
Geral). A interface é escrita em turco e inglês; os outros idiomas partiram do texto da comunidade do
Files e, a partir dali, são traduzidos por máquina. Árabe, hebraico e persa são traduzidos, e nomes e
textos da direita para a esquerda são lidos da direita para a esquerda (obrigado a @ialhabbal); a
interface em si ainda não é espelhada. Correções são bem-vindas: cada idioma é um arquivo JSON em
`crates/rf-app/locales/`.

<img alt="rustyfiles mostrando uma pasta com papéis de parede do Windows em modo grade, um deles selecionado e exibido no painel de pré-visualização à direita, sobre o fundo HyperGlass" src="docs/readme/grid.png" width="100%">

## DirectLook

Selecione um arquivo e pressione <kbd>Espaço</kbd>. O DirectLook abre uma janela de pré-visualização:
vídeo e áudio (com player próprio), imagens, textos e código, o conteúdo de arquivos compactados e um
cartão com detalhes para todo o resto. As setas do teclado percorrem a pasta enquanto ela continua
aberta.

Ele roda como programa próprio, `directlook.exe`, então um vídeo pesado nunca pesa no gerenciador de
arquivos, e nenhum dos dois consegue derrubar o outro. Funciona no rustyfiles e, se você ativar, também
no Explorador e na área de trabalho. As configurações ficam em Configurações › DirectLook.

## Ferry

Envie arquivos e pastas para o rustyfiles dos seus outros computadores na mesma rede, do mesmo jeito
que o AirDrop: clique com o botão direito › **Enviar com Ferry**.

<img alt="Configurações › Ferry com dois dispositivos próximos, um PC Windows pareado com a velocidade medida e um MacBook com uma velocidade estimada; uma notificação do Windows pergunta se deve aceitar arquivos do MacBook, e a barra de status mostra uma transferência em 61%" src="docs/readme/ferry.png" width="100%">

<sub>Um desenho de Configurações › Ferry com dispositivos inventados, com as palavras e o layout do próprio app.</sub>

- Criptografado de ponta a ponta (QUIC com TLS 1.3), direto de dispositivo para dispositivo: sem
  servidor, sem conta, nada sai da sua rede.
- Pareie seus dispositivos do mesmo jeito que pareia Bluetooth: os dois mostram o mesmo código de seis
  dígitos. Os arquivos de um dispositivo pareado podem chegar sem perguntar; qualquer outro é perguntado
  antes, em uma notificação.
- Você escolhe quem pode ver seu PC: ninguém, seus dispositivos pareados (o padrão) ou todos, por dez
  minutos. Enquanto Configurações › Ferry estiver aberta, todos podem vê-lo, para que um novo dispositivo
  consiga encontrá-lo.
- Uma transferência interrompida continua de onde parou, e cada arquivo é verificado antes de receber o
  nome definitivo. Arquivos recebidos são marcados como baixados, então o SmartScreen pergunta antes que
  um programa entre eles seja executado.
- Cada dispositivo mostra uma velocidade estimada, e **Testar velocidade** mede nos dois sentidos em
  alguns segundos.
- O app para macOS, em breve, fala o mesmo protocolo, que está documentado em
  [docs/nearby-protocol.md](docs/nearby-protocol.md).

O Ferry fica desligado até você ligá-lo em Configurações › Ferry. O Firewall do Windows pode perguntar
sobre ele: permita para redes privadas.

## Experimental: HyperGlass, o Dock e a área de trabalho

Estes recursos vêm desligados por padrão e ainda estão no início. Foram construídos e testados em um
PC com dois monitores, e algumas partes quase não foram usadas. Espere arestas ásperas e, por favor,
relate o que encontrar.

<img alt="Uma área de trabalho com o Dock do rustyfiles na parte de baixo (três ilhas: Iniciar e busca, os apps fixados e abertos, a bandeja e o relógio), o painel Iniciar do Dock aberto à esquerda com apps fixados e arquivos recentes, HyperSearch no meio com os apps encontrados para adobe, e a central de controle à direita com Wi-Fi, Bluetooth, Ethernet, a música tocando, volume e brilho" src="docs/readme/shell.png" width="100%">

<sub>Iniciar, HyperSearch e a central de controle, montados a partir de três capturas da mesma área de trabalho (elas não abrem todas ao mesmo tempo).</sub>

- **HyperGlass** (Configurações › Aparência) é um quinto fundo: o fundo da janela é um vidro que dobra
  e embaça o que está atrás dele.
- **O Dock** (Configurações › Dock, um único interruptor) coloca três ilhas de vidro na parte de baixo
  de cada monitor, no lugar da barra de tarefas do Windows: um painel Iniciar e busca próprios, os apps
  abertos e fixados com prévias de janelas, os ícones da bandeja, uma central de controle e um painel de
  notificações. Enquanto ele estiver ligado, a barra de tarefas do Windows fica oculta, um toque na tecla
  Windows abre o Iniciar do Dock e <kbd>Alt</kbd>+<kbd>Tab</kbd> abre o seletor dele. Desligue o Dock e
  as do Windows voltam.
- **A área de trabalho** desenhada pelo rustyfiles vem com o Dock: seu papel de parede e seus ícones,
  com uma opção de cantos arredondados.

O Explorador de Arquivos continua rodando por baixo de tudo isso; nada do Windows é removido ou
substituído no disco.

### Brilho para cada monitor

O Windows tem um controle de brilho só para a tela do próprio notebook. A central de controle do Dock
tem um para **cada monitor**: ligue em Configurações › Dock e depois clique no ícone de rede ou volume à
direita do Dock. Monitores externos são ajustados por **DDC/CI**, o padrão que quase todo monitor de
mesa dos últimos quinze anos fala por DisplayPort, HDMI e DVI. Se um monitor não tiver controle, verifique
se o DDC/CI está ativado no menu do próprio monitor; alguns hubs, chaves KVM e adaptadores não repassam
esse sinal. A tela de um notebook é ajustada do mesmo jeito que o controle próprio do Windows faz (ainda
não testado em um notebook).

## HyperDownload

Gerenciador de downloads próprio do rustyfiles, um complemento: Configurações › Complementos ›
HyperDownload › **Instalar**. É um programa separado ([código-fonte](https://github.com/gelenmedya/hyperdownload), MIT),
baixado somente quando você clica em Instalar e verificado antes pelo SHA-256, e pode ser removido a
qualquer momento.

- **Baixar a partir de um link** no menu de clique com o botão direito de uma pasta: um arquivo por
  muitas conexões ao mesmo tempo, pausado e retomado de onde parou, inclusive depois de reiniciar. Cole
  um link em uma pasta e o rustyfiles oferece baixá-lo ali.
- **Torrents e links magnet**, com uma caixa para escolher quais arquivos de um torrent você quer. Um
  arquivo `.torrent` é baixado pelo próprio menu.
- **Uma página de Downloads** com todos os downloads, suas partes e seu progresso, e os concluídos; o
  progresso também aparece na Central de status.
- **Assistir enquanto baixa:** um vídeo toca no DirectLook antes de terminar de baixar.

## Instalação

A instalação leva três cliques e nenhum conhecimento técnico:

1. **[Baixe rustyfiles-win-Setup.exe](https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe)** (cerca de 50 MB).
2. Abra o arquivo. Se o Windows mostrar "O Windows protegeu o seu PC", clique em **Mais informações** e
   depois em **Executar assim mesmo**. (O instalador ainda não tem assinatura de código; esse é o único
   motivo do aviso. Os instaladores de cada versão são verificados no
   [VirusTotal](https://www.virustotal.com): os links para os relatórios estão no fim das notas de
   cada versão.)
3. Pronto. O rustyfiles abre sozinho e fica no menu Iniciar.

Nenhuma senha de administrador, nada para configurar. As atualizações são baixadas em silêncio, em
segundo plano, e aplicadas na próxima vez que você abrir o rustyfiles.

**Com o [Scoop](https://scoop.sh):**

```powershell
scoop bucket add rustyfiles https://github.com/gelenmedya/scoop-rustyfiles
scoop install rustyfiles/rustyfiles
```

**Quer que pastas e <kbd>Win</kbd>+<kbd>E</kbd> abram no rustyfiles?** Ative Configurações › Avançado ›
Definir como gerenciador de arquivos padrão. Você pode desativar a qualquer momento.

**Desinstale** como qualquer app, em Configurações do Windows › Aplicativos. Tudo o que o rustyfiles
alterou volta para o Explorador de Arquivos.

**As caixas Abrir/Salvar de outros apps falham ou travam** depois que os arquivos do rustyfiles foram
removidos sem desinstalar (um antivírus, uma atualização quebrada)? Abrir o rustyfiles de novo resolve.
Se o rustyfiles não abrir mais, este comando do PowerShell devolve a caixa de diálogo do próprio Windows
a todos os apps:

```powershell
Remove-Item -Recurse "HKCU:\Software\Classes\CLSID\{DC1C5A9C-E88A-4dde-A5A1-60F82A20AEF7}", "HKCU:\Software\Classes\CLSID\{C0B4E2F3-BA21-4773-8DBA-335EC946EB8B}"
```

Prefere não usar instalador? Um zip portátil está na página de
[Releases](https://github.com/gelenmedya/rustyfiles/releases), e também há um instalador ARM64
(`rustyfiles-win-arm64-Setup.exe`). Requer Windows 11 (o Windows 10 não foi testado). Uma versão para
macOS está a caminho, e o Linux está planejado.

**Dúvidas ou ideias?** Venha dar um oi no [Discord](https://discord.gg/K2banZ36zJ), ou visite
[rustyfiles.com](https://rustyfiles.com).

### Compilar a partir do código-fonte

Você precisa do [Rust](https://rustup.rs) 1.95 ou mais recente e das Build Tools do Visual Studio com a
carga de trabalho "Desenvolvimento para desktop com C++" (uma dependência, zstd, compila código C).

```powershell
git clone https://github.com/gelenmedya/rustyfiles
cd rustyfiles
.\scripts\install.ps1            # builds, then installs for your user
.\scripts\install.ps1 -Default   # also makes it the default file manager
.\scripts\install.ps1 -Uninstall
```

`cargo run --release -p rf-app` inicia o app sem instalá-lo.

## Teclado

| Teclas | Ação |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>T</kbd> / <kbd>Ctrl</kbd>+<kbd>W</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd> | Nova aba, fechar aba, reabrir aba fechada |
| <kbd>Ctrl</kbd>+<kbd>L</kbd> ou <kbd>Alt</kbd>+<kbd>D</kbd> | Barra de endereço |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> | Busca (<kbd>Enter</kbd> pesquisa também as subpastas) |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> ou <kbd>F1</kbd> | Paleta de comandos |
| <kbd>Ctrl</kbd>+<kbd>P</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>A</kbd> | Encontrar uma pasta visitada, encontrar uma aba |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>1</kbd> … <kbd>5</kbd> | Detalhes, grade, colunas, cartões, lista |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd>, <kbd>F6</kbd> | Painel duplo, trocar de painel |
| <kbd>Shift</kbd>+<kbd>F5</kbd> / <kbd>Shift</kbd>+<kbd>F6</kbd> | Copiar, mover para o outro painel |
| <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>D</kbd> | Duplicar |
| <kbd>Alt</kbd>+<kbd>P</kbd>, <kbd>Ctrl</kbd>+<kbd>B</kbd> | Painel de pré-visualização, barra lateral |
| <kbd>Space</kbd> | DirectLook |
| <kbd>F2</kbd> | Renomear (renomeação em massa com vários itens selecionados) |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> / <kbd>Ctrl</kbd>+<kbd>Y</kbd> | Desfazer, refazer |
| <kbd>Ctrl</kbd>+<kbd>H</kbd> | Mostrar itens ocultos |

Qualquer atalho pode ser alterado em Configurações › Ações: clique em um comando e depois pressione as
novas teclas.

## Estrutura do projeto

| Crate | O que contém |
|---|---|
| `crates/rf-app` | A interface, e os dois programas: `rustyfiles` e `directlook` (`src/look/`) |
| `crates/rf-win` | Integração com o Windows e o shell, o motor de operações de arquivo |
| `crates/rf-core` | O modelo: locais, histórico, entradas |
| `crates/rf-archive` | Operações de arquivos compactados: navegar, extrair, criar, editar |
| `crates/rf-player` | O player de vídeo e áudio do DirectLook |
| `crates/rustdecoder` | Nosso próprio motor de vídeo, em desenvolvimento: MP4/MOV (inclusive fragmentado) e MKV/WebM; H.264, HEVC, VP9 e AV1 decodificados na placa de vídeo via D3D11 Video, bit a bit idênticos ao ffmpeg / dav1d nos seus testes; ProRes e DNxHR decodificados na CPU; conversão de cores, áudio (PCM, AAC, FLAC, Opus, MP3) e sincronização A/V. Vai assumir a reprodução de vídeo do DirectLook ([projeto](docs/design/rustdecoder.md)) |

A interface é construída com o [GPUI](https://gpui.rs), o framework por trás do editor Zed. A camada
para Windows dele é um fork que acrescenta a troca GPU/CPU:
[gelenmedya/gpui-pre-windows](https://github.com/gelenmedya/gpui-pre-windows).

<details>
<summary>Variáveis de ambiente</summary>

| Variável | Efeito |
|---|---|
| `RUSTYFILES_RENDERER` = `gpu`, `cpu` ou `auto` | Forçar onde a janela é desenhada |
| `RUSTYFILES_CONFIG_DIR` = pasta | Guardar as configurações em outra pasta (uso portátil) |
| `RUSTYFILES_MULTI_INSTANCE=1` | Permitir várias cópias ao mesmo tempo (testes) |
| `RUSTYFILES_FRAME_LOG=1` | Registrar os tempos de quadro |
| `RUSTYFILES_LOOK_TRACE=1` | Registrar os tempos do DirectLook no stderr |
| `RUSTYFILES_MUTE=1` | O DirectLook toca sem som (testes) |

As configurações ficam em `%APPDATA%\rustyfiles\settings.toml`; as do DirectLook, em
`directlook.toml`, ao lado dele.

</details>

## Como é feito

O rustyfiles é escrito com ferramentas de programação com IA, e isso não é segredo. Um desenvolvedor
decide o que é construído e como deve se comportar, testa cada mudança em um PC real (dois monitores,
uso do dia a dia) e descarta o que não se sustenta. A IA escreve grande parte do código; ela não decide
o que é lançado.

O que mantém isso honesto:

- **Testes.** Mais de 3,000 testes automatizados rodam antes de cada mudança ser enviada.
- **Medições.** Os números de velocidade acima vêm de um teste embutido no app (Configurações ›
  Teste de velocidade), então qualquer pessoa pode conferi-los na própria máquina.
- **Relatos.** Os bugs vêm de quem usa o app. Cada issue recebe uma resposta, a correção é feita às
  claras, e quem relatou é convidado a testar antes de a issue ser fechada.
- **O código.** Tudo está aqui sob a licença MIT, para ler e julgar.

Por que Rust: um gerenciador de arquivos precisa continuar rápido com 100,000 arquivos em uma pasta e
nunca pode perder os dados de ninguém. Rust dá velocidade sem coletor de lixo e elimina classes inteiras
de travamentos antes do lançamento. E o rustyfiles é um app novo, escrito do zero, não uma reescrita do
Explorador de Arquivos; ele roda ao lado do Explorador e não altera nada do Windows no disco.

## Status

O rustyfiles é jovem, e o autor o usa todos os dias no lugar do Explorador de Arquivos. Um roteiro e
relatórios de teste virão em `docs/`. A versão para macOS está a caminho, construída sobre o mesmo
núcleo, e o Linux vem depois dela.

Relatórios de bug e pull requests são bem-vindos.

## Listado em

- [AlternativeTo](https://alternativeto.net/software/rustyfiles/about/), como alternativa ao Explorador de Arquivos
- [Scoop](https://scoop.sh), a partir do próprio bucket (veja [Instalação](#instalação))

## Agradecimentos

O rustyfiles se apoia no trabalho de outras pessoas e tomou emprestadas muitas ideias pelo caminho.

[Files](https://github.com/files-community/Files) mostrou o que um gerenciador de arquivos moderno para
Windows pode ser. A lista de recursos dele foi nossa lista de verificação, suas configurações e layout
moldaram os nossos, e sua abordagem para substituir o Explorador de Arquivos como gerenciador padrão
guiou a nossa. Obrigado à comunidade do Files.

[GPUI](https://gpui.rs), o framework de interface do editor [Zed](https://github.com/zed-industries/zed),
desenha cada pixel do rustyfiles na GPU e torna possível uma janela tão rápida. Obrigado à equipe do Zed
por construí-lo e compartilhá-lo, e à [Longbridge](https://github.com/longbridge/gpui-kit) pelo gpui-kit,
os componentes sobre os quais construímos.

O DirectLook segue a ideia do Quick Look do macOS, e o [QuickLook](https://github.com/QL-Win/QuickLook)
de Paddy Xu, que o levou ao Windows primeiro. O rarchive aprendeu o formato RAR a partir do código-fonte
do UnRAR de Alexander Roshal.

Construído com [windows-rs](https://github.com/microsoft/windows-rs),
[Velopack](https://velopack.io) (instalação e atualizações), ícones [Lucide](https://lucide.dev),
[zip](https://github.com/zip-rs/zip2), [sevenz-rust2](https://github.com/hasenbanck/sevenz-rust),
[tar](https://github.com/alexcrichton/tar-rs), [notify](https://github.com/notify-rs/notify)
e muitos outros crates. As imagens usam a fonte
[Bricolage Grotesque](https://github.com/ateliertriay/bricolage) de Atelier Triay,
[Instrument Sans](https://fonts.google.com/specimen/Instrument+Sans) e
[Doto](https://fonts.google.com/specimen/Doto).

## Licença

MIT, veja [LICENSE](LICENSE). O fork do GPUI mantém a licença Apache-2.0 do Zed em seu próprio
repositório.
