<p align="center">
  <b>日本語</b> ·
  <a href="README.zh-CN.md">简体中文</a> ·
  <a href="README.zh-TW.md">繁體中文</a> ·
  <a href="README.md">English</a> ·
  <a href="README.de.md">Deutsch</a> ·
  <a href="README.es.md">Español</a> ·
  <a href="README.pt-BR.md">Português (Brasil)</a> ·
  <a href="README.ru.md">Русский</a> ·
  <a href="README.tr.md">Türkçe</a>
</p>

<p align="center"><sub>これは翻訳です。正確な内容は英語版の <a href="README.md">README</a> を参照してください。</sub></p>

<img alt="rustyfiles: Windows と macOS のためのファイルマネージャー。フリーズしない。Rust で書かれている。10万ファイルのフォルダーを開いた rustyfiles のウィンドウを描いたもので、281 ms で開いた" src="docs/readme/cover.png" width="100%">

<p align="center">
  <b>ディスクを待つことのないファイルマネージャー。</b><br>
  タブ、デュアルペイン、フォルダーのように開ける書庫、<kbd>Space</kbd> でのプレビュー。<br>
  Rust でゼロから書かれ、Windows、そして今では macOS のためのものです。
</p>

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest"><img alt="最新バージョン" src="https://img.shields.io/github/v/release/gelenmedya/rustyfiles?label=download&labelColor=232326&color=ff6a1a"></a>
  <a href="https://github.com/gelenmedya/rustyfiles/releases"><img alt="ダウンロード数" src="https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fgelenmedya%2Frustyfiles%2Fbadges%2Fdownloads.json"></a>
  <img alt="Windows 11、x64 と ARM64" src="https://img.shields.io/badge/Windows_11-x64_%7C_ARM64-3d3d44?labelColor=232326">
  <img alt="macOS 26、Apple silicon と Intel、ベータ版" src="https://img.shields.io/badge/macOS_26-Apple_silicon_and_Intel_(beta)-3d3d44?labelColor=232326">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest"><img alt="すべてのリリースは VirusTotal でスキャンされ、そのレポートはリリースノートの末尾にリンクされている" src="https://img.shields.io/badge/VirusTotal-every_release_scanned-3d3d44?labelColor=232326"></a>
  <a href="LICENSE"><img alt="MIT ライセンス" src="https://img.shields.io/badge/license-MIT-3d3d44?labelColor=232326"></a>
</p>

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe"><img alt="Windows 版をダウンロード" src="https://img.shields.io/badge/Download_for_Windows-FF6A1A?style=for-the-badge&logo=windows11&logoColor=white"></a>
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-mac.dmg"><img alt="macOS 版（ベータ）をダウンロード" src="https://img.shields.io/badge/Download_for_macOS_(beta)-232326?style=for-the-badge&logo=apple&logoColor=white"></a>
  <br>
  <a href="https://rustyfiles.com"><img alt="ウェブサイト" src="https://img.shields.io/badge/rustyfiles.com-232326?style=flat-square&logo=googlechrome&logoColor=white"></a>
  <a href="https://discord.gg/K2banZ36zJ"><img alt="Discord に参加" src="https://img.shields.io/badge/Discord-5865F2?style=flat-square&logo=discord&logoColor=white"></a>
  <a href="https://www.buymeacoffee.com/ozerguzel"><img alt="コーヒーをおごる" src="https://img.shields.io/badge/Buy_me_a_coffee-FFDD00?style=flat-square&logo=buymeacoffee&logoColor=black"></a>
</p>

<table>
  <tr>
    <td width="33%" valign="top">
      <b>ディスクを待たない</b><br>
      一覧表示、アイコン、サムネイル、コンテキストメニュー、そしてすべてのコピーは、ウィンドウのスレッドから離れて動きます。
      遅いネットワーク共有や10万ファイルのフォルダーでも、固まることはありません。
    </td>
    <td width="33%" valign="top">
      <b>Explorer の仕事を、より速く</b><br>
      タブ、デュアルペイン、書庫のフォルダー化、即時検索のための名前インデックス、一括名前変更、元に戻す操作、
      そして独自のコピーエンジン。小さいファイルでは14倍速い。
    </td>
    <td width="33%" valign="top">
      <b>無料で、あなたのもの</b><br>
      アカウント不要、広告なし、どこにも送信しない。MIT ライセンスで、ここにあるすべての行を読めます。
      更新はバックグラウンドで静かにインストールされます。
    </td>
  </tr>
</table>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/readme/projects-dark.png">
  <img alt="Projects フォルダーを詳細表示で開いた rustyfiles。タブ、パンくずバー、サイドバーに固定されたフォルダーがあり、ウィンドウはデスクトップの壁紙の上のガラスになっている" src="docs/readme/projects-light.png" width="100%">
</picture>

<sub>Windows、実験的な HyperGlass 背景付き（設定 › 外観）。既定は Explorer と同じ Mica です。</sub>

## 速さ

<img alt="rustyfiles の一覧で描いた速度テスト。100,000 ファイルのフォルダーを開くのに rustyfiles は 281 ms、File Explorer は 2,318 ms（8倍速い）。2,000 枚の写真のフォルダーは 278 ms 対 2,177 ms（8倍）。10,000 個の小さいファイルのコピーは 1.8 秒 対 25.6 秒（14倍）。10,000 ファイルの ZIP の展開は 3.6 秒 対 32.4 秒（9倍）" src="docs/readme/speed.png" width="100%">

10万ファイルのフォルダーは 281 ms で開きます（File Explorer: 2.3 秒）。数値はアプリに組み込まれた速度テスト（設定 › 速度テスト）から取ったもので、お使いの PC でも確かめられます。

<details>
<summary><b>すべての測定値と、その取り方</b></summary>

#### フォルダーを開く

アプリを起動してから、新しいウィンドウでフォルダーの一覧が動かなくなるまでの時間。フォルダーは OS のファイルキャッシュに入っている状態です。ウォームアップの後の3回の中央値。

| フォルダー | rustyfiles | File Explorer | [Files](https://files.community) |
|---|--:|--:|--:|
| 空のフォルダー | 281 ms | 1,006 ms | 813 ms |
| 1,000 ファイル | 284 ms | 2,214 ms | 1,893 ms |
| 10,000 ファイル | 286 ms | 2,221 ms | 1,053 ms |
| 100,000 ファイル | 281 ms | 2,318 ms | 4,069 ms |
| 2,000 枚の写真（サムネイル） | 278 ms | 2,177 ms | 1,075 ms |
| 10,000 ファイルを一覧した後のメモリ | 39 MB | 69 MB | 204 MB |

1台のマシン（Ryzen 7 9800X3D、NVMe SSD、Windows 11）で、2026-11-16 に計測。rustyfiles は 0.8.6、Files は 4.3。rustyfiles と Explorer はそれぞれ新しいプロセスを起動しました。Files はすでにバックグラウンドで動いていたため、その時間はコールドスタートのものではありません。また Files は一覧を段階的に埋めるので、時間は画面が変化しなくなるまで数えています。メモリはタスクマネージャーが表示するプライベートワーキングセットです。「バックグラウンドで実行し続ける」をオンにすると、新しい rustyfiles のウィンドウはプロセスを起動せず、一覧表示されたフォルダーを 93〜168 ms で表示しました（2026-11-06、以前のバージョン）。

#### 変化し続ける10万ファイルのフォルダー

フォルダーを開き、1ファイルを選択し、5秒間バックグラウンドで毎秒約1,000ファイルの名前を変更した状態:

| | |
|---|--:|
| 起動から一覧表示まで（100,001 ファイル） | 315 ms |
| 一覧は変更に追随する | 約750 msごと |
| フォルダーの再読み込み（バックグラウンドスレッド） | 約70 ms |
| 更新ごとの UI スレッドの作業 | 7〜8 ms |
| 選択とカーソル | 選択したファイルに留まる |

同じフォルダーを7.7秒間スクロールして戻したときのフレーム時間。選択（とそのプレビューペイン）は半秒ごとに移動し、バックグラウンドで毎秒約680ファイルの名前が変わる。200 Hz のディスプレイでの計測:

| | 中央値 | p95 | p99 | 最悪 |
|---|--:|--:|--:|--:|
| UI スレッドでのフレームコスト | 2.1 ms | 4.6 ms | 7.9 ms | 16.8 ms |
| フレーム間の時間 | 3.3 ms | 5.3 ms | 9.9 ms | 17.3 ms |

7.7秒で 2,182 フレーム。60 Hz の1フレームより長い間隔が1回ありました。`RUSTYFILES_TRACE_LIVE=1` と `RUST_LOG=warn,rf_app=info` で自分で測定でき、`GPUI_FRAME_LOG=frames.tsv` でフレームを記録できます。

#### 開いているウィンドウで新しいタブ

| フォルダー | 一覧が落ち着くまでの時間 |
|---|--:|
| 1,000 ファイル | 約48 ms |
| 10,000 ファイル | 約46 ms |
| 100,000 ファイル | 約129 ms |

タブが1つ増えるごとに約3 MB のメモリを使います（1タブ: 約32 MB、25タブ: 約103 MB）。

#### ファイル名で検索

rustyfiles はローカルドライブの名前インデックスを保持し、検索はそれで答えます。ユーザーフォルダーのすべてのサブフォルダーを、3.6 million のファイルとフォルダーを対象に「rapor」で検索し、画面上の結果が変わらなくなるまで計測:

| | 時間 |
|---|--:|
| rustyfiles 0.4 | **0.38 s** |
| File Explorer | 1.8 s（4回中の最速。ほかは 11〜32 秒） |

インデックスは異なる名前をそれぞれ1回だけ保存します。4.8 million のエントリーがあっても、異なる名前が 490 thousand しかないドライブは 61 MB です。rustyfiles の実行中はフォルダー監視がインデックスを最新に保ちます。実行の合間は、NTFS の変更ジャーナル（管理者権限なしで読み取る）が変更されたフォルダーを教えてくれるので、起動時にはそれらだけを再度一覧します。ここでは C: で 0.2 秒、歩き回ると 30 秒。アプリデータ、キャッシュ、`node_modules`、`.git` は除外されます。その中で始めた検索はディスクを歩き回ります:

| `C:\Windows` 以下の `*.dll`（342,492 項目、116,851 フォルダー、キャッシュ温まった状態） | 時間 |
|---|--:|
| rustyfiles | **7.8 s** |
| `dir /s /b` | 53 s |
| PowerShell `Get-ChildItem -Recurse` | 105 s |

6つのバックグラウンド優先スレッドで検索するので、マシンの残りは反応を保ちます。また、ジャンクションは辿らないため、454 件少ない重複を見つけます。

#### スペースキーでのプレビュー（DirectLook）

| ファイル | キーを押してから内容が準備できるまで |
|---|--:|
| JPEG、3840 × 2160 | 24〜86 ms |
| 構文ハイライト付きのソースコード | 23〜44 ms |
| 7z 書庫（中身を一覧） | 19〜31 ms |
| MP3（タグ、歌詞） | 27〜32 ms |
| H.264 動画、1080p | 最初のフレームまで 146〜185 ms |

#### 書庫

自前のエンジン [rarchive](https://github.com/gelenmedya/rarchive) で RAR を読み、公式の UnRAR と比較しました（すべてのデータをデコードし、すべての CRC を確認。書庫はファイルキャッシュ内）:

| 書庫 | UnRAR | rarchive |
|---|--:|--:|
| 1.3 GB、32 MB 辞書、195 ファイル | 2.79 s | **0.28 s**（10×） |
| 400 MB ソリッド | 0.95 s | **0.21 s**（4.5×） |
| 400 MB、520 ファイル | 1.03 s | **0.063 s**（16×） |

#### コピーと展開

Windows 標準との比較（rustyfiles 0.8.6、2026-11-16）:

| | Windows | rustyfiles |
|---|--:|--:|
| 100 フォルダーに分かれた小さいファイル 10,000 個をコピー（41 MB） | 25.6 s | **1.8 s**（14×） |
| 1 GB のファイル1つをコピー | 0.61 s | 0.44 s |
| 10,000 ファイルの ZIP を展開 | 32.4 s | **3.6 s**（9×） |

小さいファイルのコピーは複数のフォルダーに分散させます。1つのフォルダーに書き込むスレッドは互いを待つためです。大きな1ファイルの速さはエンジンではなくディスクで制限されます。名前の衝突は自分で処理します（置き換え、スキップ、両方残す）。一時停止と再開ができ、すべてのコピーをバイト単位で検証でき、速度に上限を設けられます。コピーのテストスクリプトは `crates/rf-win/examples/copybench.rs` です。

すべての数値: 1台のマシン（Ryzen 7 9800X3D、NVMe SSD、Windows 11）、2026年11月。

</details>

## 主な機能

- **ブラウズ。** タブはドラッグして別のウィンドウに出せます。保存したセッション、デュアルペイン、詳細、グリッド、一覧、列の表示、グループ化、プレビューペイン、色タグ、Shelf、ネットワーク共有と FTP。
- **ファイル。** 元に戻す・やり直し、一括名前変更、フォルダーサイズのオンデマンド計算、ツリーマップ付きのディスク使用量、Explorer のコンテキストメニュー全体、他のすべてのアプリとのドラッグ＆ドロップ。
- **書庫。** ZIP、7z、RAR、tar、ISO をフォルダーのように開けます。ZIP、7z、tar は中身を編集できます。
- **検索。** ドライブの名前インデックスと、アプリ、ファイル、ブックマーク、Windows の設定を探すクイック検索（<kbd>Alt</kbd>+<kbd>Space</kbd>）。
- **Windows。** 任意で既定のファイルマネージャーに。<kbd>Win</kbd>+<kbd>E</kbd> や他のアプリの「開く」「保存」ダイアログも含む。
- **49 言語**。システムの言語に従います。

<details>
<summary><b>すべての機能を詳しく</b></summary>

**ブラウズ。** タブはドラッグして並べ替えたり、新しいウィンドウに出したりできます。ウィンドウのタブを、分割ペインを含めて名前付きのセッションとして保存し、後で開き直せます。新しいタブはホームか、今いるフォルダーで開きます（設定 › 全般）。デュアルペイン。詳細、グリッド、一覧、列の表示（一覧は Explorer と同じく列ごとに埋められます）、Explorer のコンパクト表示のようなコンパクト項目、グループ化、プレビューペイン、検索、色タグ、Shelf、ライブラリ、ネットワーク共有と FTP。Git の状態の列。フォルダーサイズはオンデマンド: 「サイズを計算」をクリックすると、再起動後も覚えており、ディスク上のサイズを表示できます。フォルダーまたはドライブの「ディスク使用量を表示」は、場所を取っているものを大きい順に一覧し、クリックして辿れるツリーマップと並べて表示します。ライブラリやギャラリーなどの仮想フォルダーは rustyfiles で開き、Windows が「この PC」に一覧するデバイス（USB 接続の電話や Phone Link）はホーム画面の「デバイス」に現れます。Windows が認識するクラウドフォルダー（OneDrive、Proton Drive、Dropbox、MEGA など）はサイドバーにあります。

**ファイル。** 名前の変更、コピー、移動、ごみ箱への削除、フォルダーの平坦化について、元に戻すとやり直しができます。一括名前変更。もう一方のペインへの複製、コピー、移動。コピーしたテキストや画像を新しいファイルとして貼り付け。Send to を含む Explorer のコンテキストメニュー全体、Explorer、デスクトップ、ブラウザー、Outlook とのドラッグ＆ドロップ、そしてタブ、固定したフォルダー、フォルダー、プログラムへのドロップ先。ファイルを他のコンピューターに送るには [Ferry](#ferry) を、ダウンロードには [HyperDownload](#hyperdownload) を使います。

**書庫。** ZIP、7z、RAR、tar 系、ISO をフォルダーのように開きます。パスワード付きの ZIP、7z、RAR も含みます。ZIP、7z、tar の中では追加、削除、名前変更ができます。ZIP、7z、tar.zst の作成はどのコアでも可能です。読み取りは自前の純 Rust エンジン [rarchive](https://github.com/gelenmedya/rarchive)（[crates.io にも](https://crates.io/crates/rarchive)）で行い、1.3 GB の WinRAR 書庫を UnRAR の10倍速くデコードします。

**Windows。** 任意で既定のファイルマネージャーに: フォルダー、<kbd>Win</kbd>+<kbd>E</kbd>、この PC、ごみ箱が rustyfiles で開きます。任意で、他のアプリの「開く」「保存」「フォルダーの選択」ダイアログも rustyfiles で開けます（設定 › 詳細設定）。そこにあるピンのボタンで、アプリごとに開始フォルダーを保持できます。フォトや他の Store アプリで開いた写真や動画は、そのフォルダーと一緒に来るので、前へ・次へが機能します。Explorer の `/select,` と `shell:` のパスも動きます。Mica、ダークモード、システムのアクセントカラー。ゲームやレンダリングが GPU を使い切ると、描画は CPU に移り、ちらつきなく戻ります。

**アドレスバーのコマンド。** Explorer と同じく、<kbd>Ctrl</kbd>+<kbd>L</kbd> を押し、`cmd`、`powershell`、`wt`、`code .` と入力して <kbd>Enter</kbd> を押します。その文字列は最初に場所（パス、ドライブ、`%VAR%`、`shell:`）として解釈されます。開いているフォルダーにその名前のフォルダーやファイルがあれば、そちらが常に優先されます。そのような場所がない場合にだけ、最初の語を名前を指定して実行のように、プログラムとして探し、開いているフォルダーで起動します。補完からは何も起動せず、管理者権限も求めません。設定 › ファイルとフォルダー › 動作のスイッチで無効にできます。

**整理。** フォルダー用のルール: 名前、拡張子、種類、サイズ、経過時間の条件と、1つの動作（移動、テンプレートから作ったフォルダーへの移動、名前の変更、ごみ箱）。手動の実行はすべての手順をプレビューに示し、「適用」まで何も動かしません。実行は元に戻せます。完全に削除されるものも、上書きされるものもありません。自動で動くルールは既定でオフです。

**検索。** クイック検索（<kbd>Alt</kbd>+<kbd>Space</kbd>）は、アプリ、ファイルとフォルダー、ブラウザーのブックマーク、Windows の設定ページを見つけます。よく開くものが先に来て、同じ文字で前に選んだものが最初の行になります。

**言語。** 49 の言語があり、システムの表示言語に従います（または設定 › 全般で選択）。インターフェースは日本語と英語で書かれています。ほかの言語は Files コミュニティの表現から始まり、そこから機械翻訳されています。右から左に読む名前とテキストは右から左に表示されます（@ialhabbal のおかげ）。インターフェース自体はまだ左右反転していません。修正は歓迎します。各言語は `crates/rf-app/locales/` にある1つの JSON ファイルです。

</details>

<img alt="rustyfiles で Windows の壁紙のフォルダーをグリッド表示し、そのうち1枚を選択して右のプレビューペインに表示している。HyperGlass の背景上" src="docs/readme/grid.png" width="100%">

## DirectLook

ファイルを選択して <kbd>Space</kbd> を押すと、動画と音声は専用のプレイヤーで、画像、テキストとコード、書庫の中身、そしてそれ以外のものは詳細カードで表示されます。フォルダーを開いたままで矢印キーを使えば、フォルダー内を移動できます。独立したプログラムとして動くので、重い動画がファイルマネージャーを重くすることはありません。有効にすると、Explorer やデスクトップでも動きます（設定 › DirectLook）。

## Ferry

同じネットワーク上の他のコンピューターにあるrustyfilesへ、ファイルとフォルダーを送れます。AirDrop のように: 右クリック › **Ferry で送る**。設定 › Ferry で有効にするまでオフです。

<img alt="設定 › Ferry。近くの2台のデバイス、測定済みの速度が出ているペア済みの Windows PC と、推定速度の MacBook。Windows の通知が MacBook からのファイルを受け入れるか尋ねており、ステータスバーには 61 パーセントの転送が表示されている" src="docs/readme/ferry.png" width="100%">

<sub>設定 › Ferry の図。架空のデバイスを使い、アプリ自身の言葉とレイアウトで描いたもの。</sub>

<details>
<summary><b>Ferry の仕組み</b></summary>

- エンドツーエンドで暗号化（TLS 1.3 付き QUIC）され、デバイスからデバイスへ直接届きます。サーバーもアカウントもなく、ネットワークの外には何も出ません。
- デバイスは Bluetooth のペアリングと同じ方法で組みます。両方が同じ6桁のコードを表示します。ペア済みのデバイスのファイルは確認なしで届きますが、それ以外は通知で先に確認されます。
- 自分のコンピューターを誰に見せるかを選べます: 誰にも見せない、ペア済みのデバイスだけ（既定）、または10分間だけ全員に。設定 › Ferry を開いている間は全員に見えるので、新しいデバイスが見つけられます。
- 途中で切れた転送は、止まったところから続きます。各ファイルは名前を確定する前に検証されます。受け取ったファイルには、ダウンロード済みの印が付くので、その中のプログラムを実行する前に SmartScreen が確認します。
- 各デバイスは推定速度を表示し、**速度をテスト** を押すと、両方向を数秒で測ります。
- macOS 版アプリは同じプロトコルを話します。その仕様は [docs/nearby-protocol.md](docs/nearby-protocol.md) に書かれています。Windows ファイアウォールが Ferry について尋ねることがあります。プライベートネットワークで許可してください。

</details>

## HyperDownload

rustyfiles 独自のダウンロードマネージャーで、アドオンです。設定 › アドオン › HyperDownload › **インストール**。1つのファイルを複数の接続で同時に取得し、止まったところで一時停止と再開ができ、トレントとマグネットリンクに対応し、ダウンロードのページがあり、完了する前から動画を DirectLook で再生できます。独立したプログラムです（[ソース](https://github.com/gelenmedya/hyperdownload)、MIT）。インストールをクリックしたときだけダウンロードされ、まず SHA-256 で確認され、いつでも削除できます。

## Windows での実験的機能: HyperGlass、Dock、デスクトップ

既定ではオフで、まだ初期段階です。2台のモニターを持つ1台の PC で作って試しました。粗いところがあるかもしれません。見つけたことを報告してください。Explorer は下で動き続けます。Windows のものはディスク上で削除も置き換えもされません。

<img alt="rustyfiles の Dock が下部にあるデスクトップ（3つのアイランド: スタートと検索、固定と実行中のアプリ、トレイと時計）。左には Dock のスタートパネルが開き、固定したアプリと最近のファイルが表示されている。中央には adobe で見つかったアプリを表示する HyperSearch。右にはコントロールセンターがあり、Wi-Fi、Bluetooth、イーサネット、再生中の曲、音量と明るさがある" src="docs/readme/shell.png" width="100%">

<sub>スタート、HyperSearch、コントロールセンター。同じデスクトップの3枚のスクリーンショットをまとめたもので、すべてが同時に開くわけではありません。</sub>

<details>
<summary><b>各部分の役割</b></summary>

- **HyperGlass**（設定 › 外観）は5つ目の背景です。ウィンドウの背景がガラスになり、その後ろにあるものを曲げてすりガラスにします。
- **Dock**（設定 › Dock、1つのスイッチ）は、Windows のタスクバーの代わりに、各モニターの下部に3つのガラスのアイランドを置きます。独自のスタートパネルと検索、ウィンドウのプレビュー付きの実行中と固定のアプリ、トレイのアイコン、コントロールセンター、通知パネルです。オンの間は、Windows キーを1回押すと Dock のスタートが開き、<kbd>Alt</kbd>+<kbd>Tab</kbd> で Dock の切り替えが開きます。Dock をオフにすると Windows のものが戻ります。
- **rustyfiles が描くデスクトップ**は Dock と一緒に来ます。壁紙とアイコンがあり、角を丸くするオプションがあります。
- **すべてのモニターの明るさ。** Windows は、ノートPCの画面にしかスライダーを持っていません。Dock のコントロールセンターは、モニターごとにスライダーを持ちます。外部モニターは DDC/CI で設定します。過去15年のほとんどのデスクトップモニターは、DisplayPort、HDMI、DVI 上でこれを話します。モニターにスライダーがない場合は、そのモニター自身のメニューで DDC/CI がオンかを確認してください。一部のドックや KVM スイッチ、アダプターはこれを通しません。

</details>

## macOS（ベータ版）

同じファイルマネージャーを、同じ Rust のコアの上に AppKit と Liquid Glass でネイティブに描いたものです。タブ、デュアルペイン、すべての表示、<kbd>Space</kbd> での Quick Look、フォルダーのように開ける書庫、一括名前変更、ディスク使用量、Shelf、色タグ、Git の状態、サーバーへの接続、Ferry、そして同じ49言語。Dock、デスクトップ、HyperSearch、DirectLook は当面 Windows にあります。HyperDownload の Mac 版は、独自のリリースで後から続きます。Apple silicon と Intel の Mac、macOS 26（Tahoe）。Windows 版と同じように自分で更新されます。

## インストール

**Windows。** [rustyfiles-win-Setup.exe をダウンロード](https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe)（約 50 MB）して開きます。Windows に「Windows によって PC が保護されました」と表示されたら、**詳細情報** をクリックしてから **実行** をクリックしてください。インストーラーはまだコード署名されておらず、すべてのリリースは [VirusTotal](https://www.virustotal.com) でスキャンされています（レポートはリリースノートの末尾にリンクされています）。rustyfiles は自動で開き、スタートメニューに入ります。管理者パスワードは不要で、更新はバックグラウンドで静かにインストールされます。

**macOS（ベータ版）。** [rustyfiles-mac.dmg をダウンロード](https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-mac.dmg)して開き、rustyfiles を「アプリケーション」にドラッグします。Apple による署名と公証があるので、ほかの Mac アプリと同じように開きます。

<details>
<summary><b>Scoop、ポータブル、ARM64、既定のファイルマネージャー、アンインストール</b></summary>

**[Scoop](https://scoop.sh) を使う場合:**

```powershell
scoop bucket add rustyfiles https://github.com/gelenmedya/scoop-rustyfiles
scoop install rustyfiles/rustyfiles
```

**ポータブル版または ARM64 版:** ポータブルの zip と ARM64 版インストーラー（`rustyfiles-win-arm64-Setup.exe`）が [Releases](https://github.com/gelenmedya/rustyfiles/releases) ページにあります。Windows 11 が必要です（Windows 10 では未検証）。Linux は計画中です。

**rustyfiles でフォルダーと <kbd>Win</kbd>+<kbd>E</kbd>:** 設定 › 詳細設定 › 既定のファイルマネージャーとして設定 をオンにします。いつでもオフにできます。

**アンインストール**は、Windows の設定 › アプリから、ほかのアプリと同じように行います。rustyfiles が変更したものはすべて File Explorer に戻ります。

**アンインストールせずに rustyfiles のファイルが削除された後（ウイルス対策ソフト、壊れた更新）、他のアプリの「開く・保存」ダイアログが失敗する、またはクラッシュする?** rustyfiles を再度起動すると直ります。それでも起動しない場合は、次の PowerShell コマンドで、すべてのアプリに Windows 標準のダイアログが戻ります:

```powershell
Remove-Item -Recurse "HKCU:\Software\Classes\CLSID\{DC1C5A9C-E88A-4dde-A5A1-60F82A20AEF7}", "HKCU:\Software\Classes\CLSID\{C0B4E2F3-BA21-4773-8DBA-335EC946EB8B}"
```

</details>

<details>
<summary><b>ソースからのビルド</b></summary>

[Rust](https://rustup.rs) 1.95 以降と、「C++ によるデスクトップ開発」ワークロードの付いた Visual Studio Build Tools が必要です（依存関係の zstd が C のコードをコンパイルします）。

```powershell
git clone https://github.com/gelenmedya/rustyfiles
cd rustyfiles
.\scripts\install.ps1            # ビルドしてから、あなたのユーザー用にインストール
.\scripts\install.ps1 -Default   # 既定のファイルマネージャーにもする
.\scripts\install.ps1 -Uninstall
```

`cargo run --release -p rf-app` は、インストールせずに起動します。macOS アプリは `macos/` と `crates/rf-mac` にあり、Windows のワークスペースの外にあります。

</details>

**質問やアイデアはありますか?** [Discord](https://discord.gg/K2banZ36zJ) で声をかけてください。または [rustyfiles.com](https://rustyfiles.com) をご覧ください。

## 内部構造

<details>
<summary><b>キーボード</b></summary>

| キー | 動作 |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>T</kbd> / <kbd>Ctrl</kbd>+<kbd>W</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd> | 新しいタブ、タブを閉じる、閉じたタブを開き直す |
| <kbd>Ctrl</kbd>+<kbd>L</kbd> または <kbd>Alt</kbd>+<kbd>D</kbd> | アドレスバー |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> | 検索（<kbd>Enter</kbd> でサブフォルダーも検索） |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> または <kbd>F1</kbd> | コマンドパレット |
| <kbd>Ctrl</kbd>+<kbd>P</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>A</kbd> | 開いたことのあるフォルダーを探す、タブを探す |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>1</kbd> … <kbd>5</kbd> | 詳細、グリッド、列、カード、一覧 |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd>、<kbd>F6</kbd> | デュアルペイン、ペインの切り替え |
| <kbd>Shift</kbd>+<kbd>F5</kbd> / <kbd>Shift</kbd>+<kbd>F6</kbd> | もう一方のペインへコピー、移動 |
| <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>D</kbd> | 複製 |
| <kbd>Alt</kbd>+<kbd>P</kbd>、<kbd>Ctrl</kbd>+<kbd>B</kbd> | プレビューペイン、サイドバー |
| <kbd>Space</kbd> | DirectLook |
| <kbd>F2</kbd> | 名前の変更（複数選択時は一括名前変更） |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> / <kbd>Ctrl</kbd>+<kbd>Y</kbd> | 元に戻す、やり直し |
| <kbd>Ctrl</kbd>+<kbd>H</kbd> | 隠し項目を表示 |

どのショートカットも、設定 › 操作 で変更できます。コマンドをクリックしてから、新しいキーを押してください。

</details>

<details>
<summary><b>プロジェクトの構成</b></summary>

| クレート | 内容 |
|---|---|
| `crates/rf-app` | Windows のインターフェースと、その2つのプログラム `rustyfiles` と `directlook`（`src/look/`） |
| `crates/rf-win` | Windows とシェルの統合、ファイル操作エンジン |
| `crates/rf-core` | 両プラットフォームで共有するモデル: 場所、履歴、エントリー、形式 |
| `crates/rf-archive` | 書庫の操作: 閲覧、展開、作成、編集 |
| `crates/rf-nearby` | Ferry。macOS アプリと共有 |
| `crates/rf-player` | DirectLook の動画と音声のプレイヤー |
| `crates/rustdecoder` | 自前の動画エンジン、作業中: MP4/MOV と MKV/WebM。H.264、HEVC、VP9、AV1 は D3D11 Video を通してグラフィックスカードでデコードし、そのテストでは ffmpeg / dav1d とビット単位で一致。ProRes と DNxHR は CPU で。音声と A/V の同期（[設計](docs/design/rustdecoder.md)） |
| `macos/`、`crates/rf-mac` | macOS アプリ（AppKit）と Rust コアへのブリッジ |

Windows のインターフェースは [GPUI](https://gpui.rs) で作られています。Zed エディターを支えるフレームワークです。その Windows 層は、GPU/CPU の切り替えを加えたフォークです:
[gelenmedya/gpui-pre-windows](https://github.com/gelenmedya/gpui-pre-windows)。

</details>

<details>
<summary><b>環境変数</b></summary>

| 変数 | 効果 |
|---|---|
| `RUSTYFILES_RENDERER` = `gpu`、`cpu` または `auto` | ウィンドウを描く場所を強制する |
| `RUSTYFILES_CONFIG_DIR` = フォルダー | 設定を別のフォルダーに保存する（ポータブル利用） |
| `RUSTYFILES_MULTI_INSTANCE=1` | 同時に複数起動することを許可する（テスト用） |
| `RUSTYFILES_FRAME_LOG=1` | フレーム時間を記録する |
| `RUSTYFILES_LOOK_TRACE=1` | DirectLook の時間を stderr に記録する |
| `RUSTYFILES_MUTE=1` | DirectLook を無音で再生する（テスト用） |

設定は `%APPDATA%\rustyfiles\settings.toml` に保存され、DirectLook の設定はその隣の `directlook.toml` にあります。

</details>

<details>
<summary><b>作り方</b></summary>

rustyfiles は AI のコーディングツールで書かれています。それは秘密ではありません。1人の開発者が、何を作るか、どう振る舞うべきかを決め、各変更を実際の PC（2台のモニター、普段の仕事）で試し、うまくいかないものは取り下げます。コードの多くは AI が書きますが、何を出荷するかは AI が決めるわけではありません。

それを誠実に保つもの:

- **テスト。** すべての変更をプッシュする前に、4,000 を超える自動テストが走ります。
- **計測。** 速度の数値は、アプリに組み込まれたテスト（設定 › 速度テスト）から取ったもので、誰でも自分のマシンで確かめられます。
- **報告。** バグは、それを使う人たちから届きます。各 issue には返信があり、修正は公開された場で行われ、報告者は閉じる前に試すよう頼まれます。
- **コード。** そのすべてが MIT ライセンスの下でここにあり、読んで判断できます。

なぜ Rust か: ファイルマネージャーは、1つのフォルダーに10万ファイルがあっても速く、誰のデータも決して失ってはいけません。Rust はガベージコレクタなしで速さをもたらし、出荷の前に、クラッシュの種類を丸ごと排除します。rustyfiles は Explorer の書き直しではなく、ゼロから書かれた新しいアプリです。Explorer の横で動き、ディスク上の Windows には何も変更しません。

</details>

## 現状

rustyfiles はまだ若く、作者は Explorer の代わりに毎日使っています。macOS 版は同じコア上の最初のベータで、Linux はその後に来ます。バグ報告とプルリクエストを歓迎します。

## 掲載先

- [AlternativeTo](https://alternativeto.net/software/rustyfiles/about/)（File Explorer の代替として）
- [Awesome GPUI](https://github.com/zed-industries/awesome-gpui#productivity)（Productivity の項目）
- [Awesome Rust](https://github.com/rust-unofficial/awesome-rust#file-manager)（File manager の項目）
- [Scoop](https://scoop.sh)（独自のバケットから。[インストール](#インストール)を参照）

## 謝辞

rustyfiles は他の人々の仕事の上に立ち、その途中で多くのアイデアを借りました。

[Files](https://github.com/files-community/Files) は、Windows の現代的なファイルマネージャーがどのようなものになり得るかを示しました。その機能一覧は私たちのチェックリストであり、その設定と配置は私たちのものを形作り、Explorer を既定のファイルマネージャーとして置き換えるという方針は、私たちの方向を導きました。Files コミュニティに感謝します。

[GPUI](https://gpui.rs)、[Zed](https://github.com/zed-industries/zed) エディターの UI フレームワークは、Windows 上で rustyfiles のすべてのピクセルを GPU で描き、このような速さのウィンドウを可能にしています。それを作って共有してくれた Zed チームに、そして私たちが基にしている部品である gpui-kit を作った [Longbridge](https://github.com/longbridge/gpui-kit) に感謝します。

DirectLook は macOS の Quick Look の考え方に従い、Windows にそれをまず持ち込んだ Paddy Xu の [QuickLook](https://github.com/QL-Win/QuickLook) に従っています。rarchive は、Alexander Roshal による UnRAR のソースから RAR 形式を学びました。

[windows-rs](https://github.com/microsoft/windows-rs)、[Velopack](https://velopack.io)（インストールと更新）、[Lucide](https://lucide.dev) アイコン、[zip](https://github.com/zip-rs/zip2)、[sevenz-rust2](https://github.com/hasenbanck/sevenz-rust)、[tar](https://github.com/alexcrichton/tar-rs)、[notify](https://github.com/notify-rs/notify) などの多くのクレートで作られています。画像には、Atelier Triay による [Bricolage Grotesque](https://github.com/ateliertriay/bricolage)、[Instrument Sans](https://fonts.google.com/specimen/Instrument+Sans)、[Doto](https://fonts.google.com/specimen/Doto) を使っています。

## ライセンス

MIT。[LICENSE](LICENSE) を参照してください。GPUI フォークは、Zed の Apache-2.0 ライセンスを独自のリポジトリで保持しています。
