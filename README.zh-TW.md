<p align="center">
  <a href="README.md">English</a> ·
  <a href="README.zh-CN.md">简体中文</a> ·
  <b>繁體中文</b> ·
  <a href="README.ja.md">日本語</a> ·
  <a href="README.de.md">Deutsch</a> ·
  <a href="README.es.md">Español</a> ·
  <a href="README.pt-BR.md">Português (Brasil)</a> ·
  <a href="README.ru.md">Русский</a> ·
  <a href="README.tr.md">Türkçe</a>
</p>

<p align="center"><sub>本文為譯文，以英文版 <a href="README.md">README</a> 為準。</sub></p>

<img alt="rustyfiles：永不凍結的 Windows 檔案管理員，以 Rust 撰寫。圖中是 rustyfiles 視窗開啟一個有 100,000 個檔案的資料夾，耗時 281 毫秒" src="docs/readme/cover.png" width="100%">

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest"><img alt="最新版本" src="https://img.shields.io/github/v/release/gelenmedya/rustyfiles?label=download&labelColor=232326&color=ff6a1a"></a>
  <img alt="Windows 11，x64 與 ARM64" src="https://img.shields.io/badge/Windows_11-x64_%7C_ARM64-3d3d44?labelColor=232326">
  <a href="LICENSE"><img alt="MIT 授權" src="https://img.shields.io/badge/license-MIT-3d3d44?labelColor=232326"></a>
</p>

rustyfiles 是以 Rust 從頭寫成的 Windows 檔案管理員，目的是在日常工作中取代檔案總管：分頁、雙窗格、可以像資料夾一樣開啟的壓縮檔，以及一個類似 Quick Look、名為 DirectLook 的預覽工具。

視窗永遠不會等待磁碟。列出檔案、圖示、縮圖、右鍵選單，以及每一項檔案操作，都在 UI 執行緒之外進行，因此慢速的網路共用或有十萬個檔案的資料夾，都不會讓它凍結。

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe"><img alt="下載 Windows 版" src="https://img.shields.io/badge/Download_for_Windows-FF6A1A?style=for-the-badge&logo=windows11&logoColor=white"></a>
  <a href="https://rustyfiles.com"><img alt="網站" src="https://img.shields.io/badge/rustyfiles.com-232326?style=for-the-badge&logo=googlechrome&logoColor=white"></a>
  <a href="https://discord.gg/K2banZ36zJ"><img alt="加入 Discord" src="https://img.shields.io/badge/Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white"></a>
</p>

<p align="center">
  <a href="https://www.buymeacoffee.com/ozerguzel"><img alt="請我喝杯咖啡" src="https://cdn.buymeacoffee.com/buttons/v2/default-yellow.png" height="44"></a>
</p>

<p align="center">
  <a href="https://alternativeto.net/software/rustyfiles/about/?utm_source=badge&utm_medium=referral">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://alternativeto.net/static/badges/badge-compact-dark.svg">
      <img alt="rustyfiles on AlternativeTo" src="https://alternativeto.net/static/badges/badge-compact-light.svg" height="54">
    </picture>
  </a>
</p>

免費、不需帳號、沒有廣告。下載、雙擊，完成：請見 [安裝](#安裝)。

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/readme/projects-dark.png">
  <img alt="rustyfiles 以詳細資料檢視顯示 Projects 資料夾，有分頁、麵包屑導覽列，側邊欄中有釘選的資料夾；視窗是覆在桌面桌布上的玻璃效果" src="docs/readme/projects-light.png" width="100%">
</picture>

<sub>圖片使用的是實驗性的 HyperGlass 背景（設定 › 外觀）。預設背景是 Mica，與檔案總管相同。</sub>

## 速度有多快

<img alt="以 rustyfiles 清單樣式繪製的速度測試：開啟一個 100,000 個檔案的資料夾，rustyfiles 需 281 毫秒，檔案總管需 2,318 毫秒（快 8 倍）；2,000 張圖片的資料夾，278 毫秒對 2,177 毫秒（8 倍）；複製 10,000 個小檔案，1.8 秒對 25.6 秒（14 倍）；解壓縮 10,000 個檔案的 ZIP，3.6 秒對 32.4 秒（9 倍）" src="docs/readme/speed.png" width="100%">

### 開啟資料夾

從啟動應用程式到資料夾的清單靜止不動所花的時間，在新視窗中開啟、資料夾已在作業系統的檔案快取中。預熱後跑三次取中位數，以 rustyfiles 內建的速度測試量測（設定 › 速度測試），任何人都能在自己的電腦上執行。

| 資料夾 | rustyfiles | 檔案總管 | [Files](https://files.community) |
|---|--:|--:|--:|
| 空資料夾 | 281 ms | 1,006 ms | 813 ms |
| 1,000 個檔案 | 284 ms | 2,214 ms | 1,893 ms |
| 10,000 個檔案 | 286 ms | 2,221 ms | 1,053 ms |
| 100,000 個檔案 | 281 ms | 2,318 ms | 4,069 ms |
| 2,000 張圖片（縮圖） | 278 ms | 2,177 ms | 1,075 ms |
| 列出 10,000 個檔案後的記憶體 | 39 MB | 69 MB | 204 MB |

量測於同一台機器（Ryzen 7 9800X3D、NVMe SSD、Windows 11），日期為 2026-11-16，rustyfiles 版本 0.8.6，Files 4.3（0.8.5 在同一測試中約需 540 毫秒：它的第一個分頁會播放開啟動畫，0.8.6 啟動時已不再播放）。rustyfiles 與檔案總管各自啟動新的程序。Files 原本就在背景執行，因此它的時間並非冷啟動的時間；它的清單也是分批填入，時間計算到畫面不再變動為止。記憶體為私有工作集，也就是工作管理員顯示的數字；Files 則是它執行程序的數字。

開啟「在背景保持執行」時，新的 rustyfiles 視窗不會啟動新程序：在較早的一次量測中（2026-11-06，較舊的版本），已列出的資料夾顯示時間為 93–168 毫秒。

### 持續變動的十萬檔案資料夾

開啟資料夾並選取一個檔案，然後在背景每秒重新命名約 1,000 個檔案，持續 5 秒：

| | |
|---|--:|
| 從啟動到列出資料夾（100,001 個檔案） | 315 ms |
| 清單跟隨變動 | 每約 750 ms |
| 重新讀取資料夾（背景執行緒） | 約 70 ms |
| 每次更新在 UI 執行緒上的工作 | 7–8 ms |
| 選取與游標 | 留在被選取的檔案上 |

視窗從不等待資料夾：重新讀取在 UI 執行緒之外進行，而更新本身的耗時不到 120 Hz 下一個畫格的時間。你可以用 `RUSTYFILES_TRACE_LIVE=1` 與 `RUST_LOG=warn,rf_app=info` 自行量測。

發生時的畫格時間：同一個資料夾以滑鼠滾輪向下再向上捲動 7.7 秒，選取項目（因此也包括預覽窗格）每半秒移動一次，同時背景每秒約有 680 個檔案被重新命名。200 Hz 螢幕。

| | 中位數 | p95 | p99 | 最差 |
|---|--:|--:|--:|--:|
| UI 執行緒上的畫格成本 | 2.1 ms | 4.6 ms | 7.9 ms | 16.8 ms |
| 畫格之間的時間 | 3.3 ms | 5.3 ms | 9.9 ms | 17.3 ms |

共 7.7 秒內 2,182 個畫格；其中一次間隔超過 60 Hz 的一個畫格時間（16.7 毫秒）。你可以用 `GPUI_FRAME_LOG=frames.tsv` 記錄自己的數據（每個畫格一行：起始時間與其成本，單位為毫秒）。

### 在已開啟的視窗中開新分頁

| 資料夾 | 清單穩定所需時間 |
|---|--:|
| 1,000 個檔案 | ~48 ms |
| 10,000 個檔案 | ~46 ms |
| 100,000 個檔案 | ~129 ms |

每多開一個分頁約佔 3 MB 記憶體（1 個分頁：約 32 MB；25 個分頁：約 103 MB）。

### 依檔名搜尋

rustyfiles 會為本機磁碟建立檔名索引，並從索引回答搜尋。在一個使用者資料夾中搜尋 "rapor"，該資料夾底下有 360 萬個檔案與資料夾，計時至畫面上的結果不再變動為止：

| | 時間 |
|---|--:|
| rustyfiles 0.4 | **0.38 秒** |
| 檔案總管 | 1.8 秒（4 次中最快的一次；其他次花了 11–32 秒） |

索引中每個不同的名稱只存一次：一個有 480 萬筆項目、但只有 49 萬個不同名稱的磁碟，佔用 61 MB。rustyfiles 執行時，資料夾監看器會讓索引保持最新。兩次執行之間，NTFS 的變更日誌（不需系統管理員權限即可讀取）會指出哪些資料夾有變動，因此啟動時只重新列出這些資料夾，而不是走遍整個磁碟：在這台電腦上，C: 只需 0.2 秒，若要走遍則需 30 秒。應用程式資料、快取、`node_modules` 與 `.git` 資料夾不在索引之內；在其中開始的搜尋會直接走訪磁碟：

`*.dll` 於 `C:\Windows` 之下：342,492 個項目，分布在 116,851 個資料夾，檔案快取已預熱。

| 方法 | 時間 |
|---|--:|
| rustyfiles | **7.8 秒** |
| `dir /s /b` | 53 秒 |
| PowerShell `Get-ChildItem -Recurse` | 105 秒 |

rustyfiles 在六個背景優先權的執行緒上搜尋，因此機器其餘部分仍保持流暢回應。它不追蹤接合點（junction），因此少找到了 454 個重複檔案。

### 按空白鍵預覽（DirectLook）

從按下按鍵到內容出現在畫面上的時間，應用程式已在執行：

| 檔案 | 內容就緒 |
|---|--:|
| JPEG，3840 × 2160 | 24–86 ms |
| 原始碼，語法上色 | 23–44 ms |
| 7z 壓縮檔（列出內容） | 19–31 ms |
| MP3（標籤、歌詞） | 27–32 ms |
| H.264 影片，1080p | 第一個畫格在 146–185 ms |

### 壓縮檔

使用我們自己的引擎 [rarchive](https://github.com/gelenmedya/rarchive) 讀取 RAR，並與官方的 UnRAR 比較（解碼並檢查每一個 CRC，壓縮檔已在檔案快取中）：

| 壓縮檔 | UnRAR | rarchive |
|---|--:|--:|
| 1.3 GB，32 MB 字典，195 個檔案 | 2.79 秒 | **0.28 秒**（10×） |
| 400 MB 固實壓縮 | 0.95 秒 | **0.21 秒**（4.5×） |
| 400 MB，520 個檔案 | 1.03 秒 | **0.063 秒**（16×） |

### 複製與解壓縮

同一項速度測試，我們自己的引擎對比 Windows 的（rustyfiles 0.8.6，2026-11-16）：

| | Windows | rustyfiles |
|---|--:|--:|
| 複製 100 個資料夾中的 10,000 個小檔案（41 MB） | 25.6 秒 | **1.8 秒**（14×） |
| 複製一個 1 GB 檔案 | 0.61 秒 | 0.44 秒 |
| 解壓縮 10,000 個檔案的 ZIP | 32.4 秒 | **3.6 秒**（9×） |

複製使用自己的平行引擎，會把小檔案分散到多個資料夾中，因為寫入同一個資料夾的執行緒會彼此等待。單一大型檔案受限於磁碟而非引擎（在較早的一次執行中，Windows 以幾毫秒之差略勝）。依名稱搜尋測試資料夾（123,000 個項目）花了 55 毫秒。

它也會自行處理名稱衝突（取代、略過或兩者都保留，可套用於全部或逐一選擇）、可以暫停與繼續、能逐位元組驗證每一份複本，還能限制速度。

所有數字都來自同一台機器（Ryzen 7 9800X3D、NVMe SSD、Windows 11），時間為 2026 年 11 月。開啟資料夾、複製與解壓縮的數字來自內建速度測試（設定 › 速度測試）；其他測試腳本位於儲存庫中（複製的測試為 `crates/rf-win/examples/copybench.rs`）。

## 功能總覽

**瀏覽。** 分頁可以拖曳重新排序，或拉出成新視窗。可以把視窗的分頁（包括分割窗格）存成具名工作階段，之後再開啟；名稱會顯示在分頁旁。新分頁會開啟首頁或你目前所在的資料夾（設定 › 一般）。雙窗格。詳細資料、格狀、清單與欄位檢視（清單可以像檔案總管一樣逐欄填滿）、分組、預覽窗格、搜尋、色彩標籤、暫存區（Shelf）、程式庫、網路共用與 FTP。Git 狀態欄。依需求計算資料夾大小：按一下「計算大小」；結果會在重新啟動後保留，也可以顯示磁碟上的大小。對資料夾或磁碟使用「顯示磁碟使用量」，會依佔用空間由大到小列出內容，旁邊是可點選瀏覽的樹狀圖（C:\Windows，34.1 萬個項目：佔用 26 MB）。程式庫與相簿等虛擬資料夾會在 rustyfiles 中開啟；Windows 在「本機」下列出的裝置（透過 USB 連接的手機或電話連結）會出現在首頁的「裝置」之下；這部分尚未用真實手機測試過。

**檔案。** 重新命名、複製、移動、刪除到資源回收筒，以及資料夾扁平化，都可以復原與重做。批次重新命名。複製、移動或重複到另一個窗格，並可把複製的文字或圖片貼上為新檔案。完整的檔案總管右鍵選單，包括「傳送到」，以及與檔案總管、桌面、瀏覽器和 Outlook 之間的拖放；分頁、釘選的資料夾、資料夾與程式都可以作為放置目標。可以透過 [Ferry](#ferry) 把檔案傳送到你的其他電腦，也能用 [HyperDownload](#hyperdownload) 下載。

**壓縮檔。** ZIP、7z、RAR、tar 系列與 ISO 都可以像資料夾一樣開啟，包括有密碼保護的 ZIP、7z 與 RAR。可在 ZIP、7z 與 tar 內新增、刪除與重新命名。在所有核心上都能建立 ZIP、7z 與 tar.zst。讀取由 [rarchive](https://github.com/gelenmedya/rarchive) 完成，這是我們自己以純 Rust 寫成的引擎（也發佈在 [crates.io](https://crates.io/crates/rarchive) 上，供其他 Rust 專案使用）：它解碼 1.3 GB 的 WinRAR 壓縮檔，速度是 UnRAR 的十倍。

**Windows。** 可選擇將 rustyfiles 設為預設檔案管理員：資料夾、<kbd>Win</kbd>+<kbd>E</kbd>、本機與資源回收筒都會在 rustyfiles 中開啟。也可選擇讓其他應用程式的「開啟」、「儲存」與「選取資料夾」對話方塊同樣在 rustyfiles 中開啟（設定 › 進階），而對話方塊中的釘選按鈕可以為每個應用程式保留一個起始資料夾。在相片或其他 Microsoft Store 應用程式中開啟的圖片與影片，會連同其所在資料夾一起開啟，因此「上一個」與「下一個」都能使用。檔案總管的 `/select,` 與 `shell:` 路徑可以運作。Mica、深色模式與系統強調色。當遊戲或算圖讓 GPU 飽和時，繪製會切換到 CPU，之後再切回，不會閃爍。

**位址列中的命令。** 與檔案總管相同：按 <kbd>Ctrl</kbd>+<kbd>L</kbd>，輸入 `cmd`、`powershell`、`wt` 或 `code .`，然後按 <kbd>Enter</kbd>。輸入的文字會先被當成一個位置來解讀（路徑、磁碟機、`%VAR%`、`shell:`）；若開啟的資料夾中有同名的資料夾或檔案，則一律以它為準。只有在沒有這樣的位置時，才會像「執行」對話方塊一樣，把第一個字當成程式來查找，並在開啟的資料夾中啟動，其餘文字作為引數。自動完成的項目不會被直接啟動，也不會要求系統管理員權限。「設定 › 檔案與資料夾 › 行為」下的開關可以將此功能關閉。

**整理。** 資料夾的規則：依名稱、副檔名、類型、大小與時間設定條件，並執行一個動作（移動、移動到依範本建立的資料夾、重新命名，或移到資源回收筒）。手動執行時，會在預覽中顯示每個步驟，按下「套用」前不會移動任何東西，而且執行可以復原。任何東西都不會被永久刪除，也不會被覆寫。會自動執行的規則預設為關閉。

**搜尋。** 快速搜尋（<kbd>Alt</kbd>+<kbd>Space</kbd>）可找到應用程式、檔案與資料夾、瀏覽器書籤，以及 Windows 的設定頁面。常開啟的項目排在前面，而你之前對相同字母所做的選擇會排在第一列。

**語言。** 共 49 種，依 Windows 顯示語言自動選擇（或在「設定 › 一般」中指定）。介面以土耳其文與英文撰寫；其他語言以 Files 社群的用詞為起點，之後再以機器翻譯。阿拉伯文、希伯來文與波斯文已翻譯，從右至左的名稱與文字會從右至左顯示（感謝 @ialhabbal），但介面本身尚未鏡像翻轉。歡迎更正：每種語言都是 `crates/rf-app/locales/` 中的一個 JSON 檔。

<img alt="rustyfiles 以格狀檢視顯示 Windows 桌布資料夾，其中一張被選取，並顯示在右側的預覽窗格中，背景為 HyperGlass" src="docs/readme/grid.png" width="100%">

## DirectLook

選取一個檔案並按 <kbd>Space</kbd>。DirectLook 會開啟一個預覽視窗：影片與音訊（附有自己的播放器）、圖片、文字與程式碼、壓縮檔的內容，以及其他所有檔案的詳細資料卡。視窗開啟時，方向鍵可以在資料夾中移動。

它以獨立程式 `directlook.exe` 執行，因此再大的影片也不會拖慢檔案管理員，兩者也不會因為對方而當機。它可在 rustyfiles 中使用；若你開啟設定，也能在檔案總管與桌面中使用。設定位於「設定 › DirectLook」。

## Ferry

像 AirDrop 一樣，把檔案與資料夾傳送到同一個網路上其他電腦的 rustyfiles：右鍵 › **以 Ferry 傳送**。

<img alt="設定 › Ferry，顯示兩個附近的裝置：一台已配對的 Windows 電腦及其實測速度，以及一台估計速度的 MacBook；Windows 通知詢問是否接受來自 MacBook 的檔案，狀態列顯示一項傳輸進度為 61%" src="docs/readme/ferry.png" width="100%">

<sub>設定 › Ferry 的示意圖，裝置為虛構，文字與版面取自應用程式本身。</sub>

- 端到端加密（QUIC 搭配 TLS 1.3），直接從裝置傳到裝置：沒有伺服器、沒有帳號，資料不會離開你的網路。
- 配對裝置的方式與配對藍牙相同：兩邊都會顯示相同的六位數代碼。已配對裝置的檔案可以不經詢問直接送達；其他的則會先以通知詢問。
- 你決定誰能看到你的電腦：沒有人、你已配對的裝置（預設），或所有人（十分鐘）。開啟「設定 › Ferry」時，所有人都看得到它，讓新裝置能找到它。
- 中斷的傳輸會從停下的地方繼續，每個檔案在取得名稱前都會先檢查。接收到的檔案會被標記為已下載，因此其中的程式在執行前，SmartScreen 會先詢問。
- 每台裝置都會顯示估計速度，**測試速度** 可在幾秒內從兩個方向量測。
- 即將推出的 macOS 應用程式使用相同的協定，該協定記載於 [docs/nearby-protocol.md](docs/nearby-protocol.md)。

在「設定 › Ferry」中開啟之前，Ferry 都是關閉的。Windows 防火牆可能會詢問，請允許它在私人網路中使用。

## 實驗性功能：HyperGlass、Dock 與桌面

這些功能預設為關閉，而且還在早期階段。它們是在一台有兩個螢幕的電腦上開發並測試的，部分功能的使用經驗非常有限。預期會有粗糙之處，發現問題請告訴我們。

<img alt="桌面上，底部是 rustyfiles Dock（三個島：開始與搜尋、釘選與執行中的應用程式、通知區域與時鐘）；左側開啟了 Dock 的開始面板，顯示釘選的應用程式與最近的檔案；中間是 HyperSearch，列出搜尋 adobe 找到的應用程式；右側是控制中心，有 Wi-Fi、藍牙、乙太網路、正在播放的歌曲、音量與亮度" src="docs/readme/shell.png" width="100%">

<sub>開始、HyperSearch 與控制中心，由同一個桌面的三張截圖拼成（它們不會同時開啟）。</sub>

- **HyperGlass**（設定 › 外觀）是第五種背景：視窗的背景是玻璃，會扭曲並模糊其後方的內容。
- **Dock**（設定 › Dock，一個開關）在每個螢幕底部放置三個玻璃島，取代 Windows 的工作列：它自有的開始面板與搜尋、執行中與釘選的應用程式（附視窗預覽）、通知區域圖示、控制中心與通知面板。開啟時，Windows 工作列會被隱藏，輕觸 Windows 鍵會開啟 Dock 的開始面板，<kbd>Alt</kbd>+<kbd>Tab</kbd> 則會開啟它的切換器。關閉 Dock，Windows 原本的就會回來。
- **桌面**由 rustyfiles 繪製，隨 Dock 一同提供：你的桌布與圖示，並可選擇圓角。

檔案總管在這一切之下持續執行；磁碟上沒有任何 Windows 的檔案被移除或取代。

### 每個螢幕都能調亮度

Windows 的亮度滑桿只適用於筆電本身的螢幕。Dock 的控制中心則為**每一個螢幕**提供滑桿：開啟「設定 › Dock」，然後點選 Dock 右側的網路或音量圖示。外接螢幕是透過 **DDC/CI** 設定的，這是過去十五年幾乎所有桌上型顯示器都透過 DisplayPort、HDMI 與 DVI 支援的標準。如果某個螢幕沒有滑桿，請檢查螢幕自身選單中的 DDC/CI 是否開啟；有些擴充座、KVM 切換器與轉接器不會傳遞這項訊號。筆電本身的螢幕則以 Windows 自己的滑桿相同的方式設定（尚未在筆電上測試）。

## HyperDownload

rustyfiles 自家的下載管理員，是一個附加元件：「設定 › 附加元件 › HyperDownload › **安裝**」。它是獨立的程式（[原始碼](https://github.com/gelenmedya/hyperdownload)，MIT 授權），只有在你按下「安裝」時才會下載，下載後會先以 SHA-256 核對，而且隨時可以移除。

- **從連結下載**：資料夾右鍵選單中的選項。以多條連線同時下載一個檔案，可暫停，並從停下的地方續傳，即使重新啟動後也可以。把連結貼到資料夾中，rustyfiles 會詢問是否要在那裡下載。
- **Torrent 與 magnet 連結**，附有對話方塊可以選擇要下載 torrent 中的哪些檔案。`.torrent` 檔案可從它自己的選單下載。
- **下載頁面**：列出所有下載、它們的分塊與進度，以及已完成的項目；進度也會顯示在狀態中心。
- **邊下載邊觀看**：影片在下載完成前就能在 DirectLook 中播放。

## 安裝

安裝只需三個步驟，不需要任何技術知識：

1. **[下載 rustyfiles-win-Setup.exe](https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe)**（約 50 MB）。
2. 開啟它。若 Windows 顯示「Windows 已保護您的電腦」，請按一下 **其他資訊**，然後按 **仍要執行**。（安裝程式尚未經過程式碼簽署，這是出現警告的唯一原因。每個版本的安裝程式都會在 [VirusTotal](https://www.virustotal.com) 上掃描：報告連結位於該版本發行說明的末尾。）
3. 就這樣。rustyfiles 會自動開啟，並出現在你的開始功能表中。

不需要系統管理員密碼，也不需要任何設定。更新會在背景靜默下載，並在你下次開啟 rustyfiles 時套用。

**使用 [Scoop](https://scoop.sh)：**

```powershell
scoop bucket add rustyfiles https://github.com/gelenmedya/scoop-rustyfiles
scoop install rustyfiles/rustyfiles
```

**想讓資料夾與 <kbd>Win</kbd>+<kbd>E</kbd> 都在 rustyfiles 中開啟嗎？** 開啟「設定 › 進階 › 設定為預設檔案管理員」。隨時都可以關閉。

**解除安裝**與一般應用程式相同，從「Windows 設定 › 應用程式」進行。rustyfiles 所做的每項變更都會還原為檔案總管。

**其他應用程式的開啟／儲存對話方塊失敗或當機**，是否發生在 rustyfiles 的檔案被移除（而非透過解除安裝）之後（病毒掃描程式、更新失敗）？重新啟動 rustyfiles 即可修復。如果 rustyfiles 無法再啟動，這段 PowerShell 命令會讓所有應用程式恢復使用 Windows 原生的對話方塊：

```powershell
Remove-Item -Recurse "HKCU:\Software\Classes\CLSID\{DC1C5A9C-E88A-4dde-A5A1-60F82A20AEF7}", "HKCU:\Software\Classes\CLSID\{C0B4E2F3-BA21-4773-8DBA-335EC946EB8B}"
```

不想使用安裝程式嗎？可攜式 zip 版本位於 [Releases](https://github.com/gelenmedya/rustyfiles/releases) 頁面，另有 ARM64 安裝程式（`rustyfiles-win-arm64-Setup.exe`）。需要 Windows 11（Windows 10 尚未測試）。macOS 版本即將推出，Linux 版本也在規劃中。

**有問題或想法？** 歡迎到 [Discord](https://discord.gg/K2banZ36zJ) 打個招呼，或造訪 [rustyfiles.com](https://rustyfiles.com)。

### 從原始碼建置

你需要 [Rust](https://rustup.rs) 1.95 或更新版本，以及安裝了「使用 C++ 的桌面開發」工作負載的 Visual Studio Build Tools（其中一個相依項目 zstd 會編譯 C 程式碼）。

```powershell
git clone https://github.com/gelenmedya/rustyfiles
cd rustyfiles
.\scripts\install.ps1            # builds, then installs for your user
.\scripts\install.ps1 -Default   # also makes it the default file manager
.\scripts\install.ps1 -Uninstall
```

`cargo run --release -p rf-app` 會在不安裝的情況下啟動它。

## 鍵盤快速鍵

| 按鍵 | 動作 |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>T</kbd> / <kbd>Ctrl</kbd>+<kbd>W</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd> | 新增分頁、關閉分頁、重新開啟已關閉的分頁 |
| <kbd>Ctrl</kbd>+<kbd>L</kbd> 或 <kbd>Alt</kbd>+<kbd>D</kbd> | 位址列 |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> | 搜尋（<kbd>Enter</kbd> 也會搜尋子資料夾） |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> 或 <kbd>F1</kbd> | 命令選擇區 |
| <kbd>Ctrl</kbd>+<kbd>P</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>A</kbd> | 尋找曾造訪的資料夾、尋找分頁 |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>1</kbd> … <kbd>5</kbd> | 詳細資料、格狀、欄位、卡片、清單 |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd>、<kbd>F6</kbd> | 雙窗格、切換窗格 |
| <kbd>Shift</kbd>+<kbd>F5</kbd> / <kbd>Shift</kbd>+<kbd>F6</kbd> | 複製、移動到另一個窗格 |
| <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>D</kbd> | 重複 |
| <kbd>Alt</kbd>+<kbd>P</kbd>、<kbd>Ctrl</kbd>+<kbd>B</kbd> | 預覽窗格、側邊欄 |
| <kbd>Space</kbd> | DirectLook |
| <kbd>F2</kbd> | 重新命名（選取多個項目時為批次重新命名） |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> / <kbd>Ctrl</kbd>+<kbd>Y</kbd> | 復原、重做 |
| <kbd>Ctrl</kbd>+<kbd>H</kbd> | 顯示隱藏項目 |

每個快速鍵都可以在「設定 › 動作」中變更：點選一個命令，然後按下新的按鍵即可。

## 專案結構

| 套件（crate） | 內容 |
|---|---|
| `crates/rf-app` | 介面，以及兩個程式：`rustyfiles` 與 `directlook`（`src/look/`） |
| `crates/rf-win` | Windows 與殼層整合，檔案操作引擎 |
| `crates/rf-core` | 模型：位置、歷史記錄、項目 |
| `crates/rf-archive` | 壓縮檔操作：瀏覽、解壓縮、建立、編輯 |
| `crates/rf-player` | DirectLook 的影片與音訊播放器 |
| `crates/rustdecoder` | 我們自己的影片引擎，開發中：MP4/MOV（也包括分段式）與 MKV/WebM；H.264、HEVC、VP9 與 AV1 透過 D3D11 Video 在顯示卡上解碼，在其測試中與 ffmpeg / dav1d 的輸出位元完全一致；ProRes 與 DNxHR 在 CPU 上解碼；色彩轉換、音訊（PCM、AAC、FLAC、Opus、MP3）與音視訊同步。之後將接手 DirectLook 的影片播放（[設計文件](docs/design/rustdecoder.md)） |

介面以 [GPUI](https://gpui.rs) 建構，它是 Zed 編輯器背後的框架。其 Windows 層是一個分支，加入了 GPU／CPU 切換功能：[gelenmedya/gpui-pre-windows](https://github.com/gelenmedya/gpui-pre-windows)。

<details>
<summary>環境變數</summary>

| 變數 | 效果 |
|---|---|
| `RUSTYFILES_RENDERER` = `gpu`、`cpu` 或 `auto` | 強制指定視窗在哪裡繪製 |
| `RUSTYFILES_CONFIG_DIR` = 資料夾 | 將設定保存在另一個資料夾（可攜式使用） |
| `RUSTYFILES_MULTI_INSTANCE=1` | 允許同時執行多個副本（測試用） |
| `RUSTYFILES_FRAME_LOG=1` | 記錄畫格時間 |
| `RUSTYFILES_LOOK_TRACE=1` | 將 DirectLook 的時間記錄輸出到 stderr |
| `RUSTYFILES_MUTE=1` | DirectLook 播放時不出聲（測試用） |

設定儲存於 `%APPDATA%\rustyfiles\settings.toml`，DirectLook 的設定則在其旁的 `directlook.toml` 中。

</details>

## 開發方式

rustyfiles 是以 AI 程式設計工具撰寫的，這並不是秘密。一位開發者決定要建造什麼、應該如何運作，在真實的電腦上（兩個螢幕，日常工作）測試每項變更，並淘汰站不住腳的部分。AI 寫出大部分的程式碼；但它不決定什麼能出貨。

確保這一切誠實的機制：

- **測試。** 每次變更推送之前，都會執行超過 3,000 項自動化測試。
- **量測。** 上方的速度數字來自應用程式內建的測試（設定 › 速度測試），因此任何人都能在自己的電腦上核對。
- **回報。** 錯誤來自使用者本身。每個問題都會得到回應，修正在公開場合進行，並且會請回報者在關閉問題前試用。
- **程式碼。** 全部都在這裡，以 MIT 授權釋出，供閱讀與評斷。

為什麼選擇 Rust：檔案管理員必須在資料夾有十萬個檔案時仍然快速，而且絕不能遺失任何人的資料。Rust 在沒有垃圾回收器的情況下提供速度，並在程式出貨前就排除了整類的當機問題。而且 rustyfiles 是從頭撰寫的新應用程式，而不是檔案總管的重寫；它與檔案總管並行執行，且不會在磁碟上改變 Windows 的任何東西。

## 目前狀態

rustyfiles 還很年輕，作者每天都用它來取代檔案總管。路線圖與測試報告將會放在 `docs/` 中。macOS 版本正在開發中，建立在同一個核心上，Linux 則排在其後。

歡迎回報錯誤與提交 Pull Request。

## 收錄於

- [AlternativeTo](https://alternativeto.net/software/rustyfiles/about/)，作為檔案總管的替代方案
- [Scoop](https://scoop.sh)，透過專屬的 bucket（見[安裝](#安裝)）

## 致謝

rustyfiles 建立在他人的成果之上，並在過程中借用了許多想法。

[Files](https://github.com/files-community/Files) 展示了現代的 Windows 檔案管理員可以是什麼樣子。它的功能清單是我們的檢查表，它的設定與版面影響了我們的設計，而它以 Files 取代檔案總管作為預設檔案管理員的做法，也指引了我們的方向。感謝 Files 社群。

[GPUI](https://gpui.rs)，即 [Zed](https://github.com/zed-industries/zed) 編輯器的 UI 框架，在 GPU 上繪製 rustyfiles 的每一個像素，使如此快速的視窗成為可能。感謝 Zed 團隊建構並分享它，也感謝 [Longbridge](https://github.com/longbridge/gpui-kit) 提供的 gpui-kit，那是我們所依賴的元件。

DirectLook 遵循 macOS Quick Look 的理念，以及 Paddy Xu 的 [QuickLook](https://github.com/QL-Win/QuickLook)，它是第一個將此理念帶到 Windows 的專案。rarchive 的 RAR 格式知識來自 Alexander Roshal 的 UnRAR 原始碼。

以 [windows-rs](https://github.com/microsoft/windows-rs)、[Velopack](https://velopack.io)（安裝與更新）、[Lucide](https://lucide.dev) 圖示、[zip](https://github.com/zip-rs/zip2)、[sevenz-rust2](https://github.com/hasenbanck/sevenz-rust)、[tar](https://github.com/alexcrichton/tar-rs)、[notify](https://github.com/notify-rs/notify) 等眾多套件建構而成，還有更多套件。圖片使用了 Atelier Triay 的 [Bricolage Grotesque](https://github.com/ateliertriay/bricolage)，以及 [Instrument Sans](https://fonts.google.com/specimen/Instrument+Sans) 與 [Doto](https://fonts.google.com/specimen/Doto)。

## 授權

MIT，詳見 [LICENSE](LICENSE)。GPUI 分支在其自己的儲存庫中保留 Zed 的 Apache-2.0 授權。
