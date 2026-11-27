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

<img alt="rustyfiles：適用於 Windows 與 macOS 的檔案管理員，永不凍結。以 Rust 撰寫。圖中是 rustyfiles 視窗顯示一個有 100,000 個檔案的資料夾，開啟時間為 281 毫秒" src="docs/readme/cover.png" width="100%">

<p align="center">
  <b>一個永不等待磁碟的檔案管理員。</b><br>
  分頁、雙窗格、像資料夾一樣開啟的壓縮檔，以及按下 <kbd>Space</kbd> 即可預覽。<br>
  以 Rust 從頭寫成，先支援 Windows，現在也支援 macOS。
</p>

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest"><img alt="最新版本" src="https://img.shields.io/github/v/release/gelenmedya/rustyfiles?label=download&labelColor=232326&color=ff6a1a"></a>
  <a href="https://github.com/gelenmedya/rustyfiles/releases"><img alt="下載次數" src="https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fgelenmedya%2Frustyfiles%2Fbadges%2Fdownloads.json"></a>
  <img alt="Windows 11，x64 與 ARM64" src="https://img.shields.io/badge/Windows_11-x64_%7C_ARM64-3d3d44?labelColor=232326">
  <img alt="搭載 Apple 晶片或 Intel 的 macOS 26，測試版" src="https://img.shields.io/badge/macOS_26-Apple_silicon_and_Intel_(beta)-3d3d44?labelColor=232326">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest"><img alt="每個版本都會在 VirusTotal 掃描：報告連結列在其發行說明結尾" src="https://img.shields.io/badge/VirusTotal-every_release_scanned-3d3d44?labelColor=232326"></a>
  <a href="LICENSE"><img alt="MIT 授權" src="https://img.shields.io/badge/license-MIT-3d3d44?labelColor=232326"></a>
</p>

<p align="center">
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe"><img alt="下載 Windows 版" src="https://img.shields.io/badge/Download_for_Windows-FF6A1A?style=for-the-badge&logo=windows11&logoColor=white"></a>
  <a href="https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-mac.dmg"><img alt="下載 macOS 版（測試版）" src="https://img.shields.io/badge/Download_for_macOS_(beta)-232326?style=for-the-badge&logo=apple&logoColor=white"></a>
  <br>
  <a href="https://rustyfiles.com"><img alt="網站" src="https://img.shields.io/badge/rustyfiles.com-232326?style=flat-square&logo=googlechrome&logoColor=white"></a>
  <a href="https://discord.gg/K2banZ36zJ"><img alt="加入 Discord" src="https://img.shields.io/badge/Discord-5865F2?style=flat-square&logo=discord&logoColor=white"></a>
  <a href="https://www.buymeacoffee.com/ozerguzel"><img alt="請我喝杯咖啡" src="https://img.shields.io/badge/Buy_me_a_coffee-FFDD00?style=flat-square&logo=buymeacoffee&logoColor=black"></a>
</p>

<table>
  <tr>
    <td width="33%" valign="top">
      <b>永不等待磁碟</b><br>
      列出檔案、圖示、縮圖、右鍵選單，以及每一次複製，都在非視窗執行緒上進行。
      慢速的網路共用或一個有 100,000 個檔案的資料夾，都不會讓它凍結。
    </td>
    <td width="33%" valign="top">
      <b>Explorer 的工作，做得更快</b><br>
      分頁、雙窗格、把壓縮檔當資料夾、即時搜尋用的名稱索引、批次重新命名、復原，
      以及它自己的複製引擎：處理小檔案時快 14 倍。
    </td>
    <td width="33%" valign="top">
      <b>免費，而且屬於你</b><br>
      不需帳號、沒有廣告，也不會傳送任何東西到別處。採用 MIT 授權，這裡每一行程式碼都可以閱讀。
      更新會在背景安靜地安裝。
    </td>
  </tr>
</table>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/readme/projects-dark.png">
  <img alt="rustyfiles 以詳細資料檢視顯示 Projects 資料夾，有分頁、麵包屑列，側欄有釘選的資料夾；視窗是覆蓋在桌面桌布上的玻璃" src="docs/readme/projects-light.png" width="100%">
</picture>

<sub>Windows，使用實驗性的 HyperGlass 背景（設定 › 外觀）。預設為 Mica，與 Explorer 相同。</sub>

## 速度有多快

<img alt="速度測試以 rustyfiles 清單的形式呈現：開啟一個有 100,000 個檔案的資料夾，rustyfiles 需 281 毫秒，File Explorer 需 2,318 毫秒（快 8 倍）；一個有 2,000 張圖片的資料夾，278 毫秒對 2,177 毫秒（8 倍）；複製 10,000 個小檔案，1.8 秒對 25.6 秒（14 倍）；解壓縮一個有 10,000 個檔案的 ZIP，3.6 秒對 32.4 秒（9 倍）" src="docs/readme/speed.png" width="100%">

開啟一個有 100,000 個檔案的資料夾只需 281 毫秒（File Explorer：2.3 秒）。這些數字來自應用程式內建的速度測試，
位於 設定 › 速度測試，你可以在自己的電腦上驗證。

<details>
<summary><b>每一項測量，以及測量方式</b></summary>

#### 開啟資料夾

從啟動應用程式到資料夾的清單停止變動的時間，在新視窗中測量，資料夾已在作業系統的檔案快取中。暖機後執行三次，取中位數。

| 資料夾 | rustyfiles | File Explorer | [Files](https://files.community) |
|---|--:|--:|--:|
| 空資料夾 | 281 毫秒 | 1,006 毫秒 | 813 毫秒 |
| 1,000 個檔案 | 284 毫秒 | 2,214 毫秒 | 1,893 毫秒 |
| 10,000 個檔案 | 286 毫秒 | 2,221 毫秒 | 1,053 毫秒 |
| 100,000 個檔案 | 281 毫秒 | 2,318 毫秒 | 4,069 毫秒 |
| 2,000 張圖片（縮圖） | 278 毫秒 | 2,177 毫秒 | 1,075 毫秒 |
| 列出 10,000 個檔案後的記憶體 | 39 MB | 69 MB | 204 MB |

於 2026-11-16 在一台電腦上測量（Ryzen 7 9800X3D、NVMe SSD、Windows 11），使用 rustyfiles
0.8.6 與 Files 4.3。rustyfiles 與 Explorer 各自啟動新的程序。Files 已在背景執行，因此它的時間並非冷啟動時間；它的清單也是分段填入的，
時間計算到畫面不再變動為止。記憶體為私有工作集，即工作管理員顯示的數值。開啟「在背景保持執行」時，新的 rustyfiles 視窗不會啟動任何程序，
並在 93–168 毫秒內顯示已列出的資料夾（2026-11-06，較早的版本）。

#### 一個持續變動的 100,000 個檔案資料夾

開啟資料夾，選取一個檔案，然後在背景中每秒約重新命名 1,000 個檔案，持續 5 秒：

| | |
|---|--:|
| 啟動到列出資料夾（100,001 個檔案） | 315 毫秒 |
| 清單跟隨變動 | 約每 750 毫秒 |
| 重新讀取資料夾（背景執行緒） | 約 70 毫秒 |
| 每次更新在 UI 執行緒上的工作 | 7–8 毫秒 |
| 選取與游標 | 停留在已選取的檔案上 |

同一個資料夾捲動到底再捲回來，歷時 7.7 秒，選取項目（以及預覽窗格）每半秒移動一次，背景每秒約重新命名 680 個檔案，
在 200 Hz 的螢幕上的影格時間：

| | 中位數 | p95 | p99 | 最差 |
|---|--:|--:|--:|--:|
| UI 執行緒上的影格成本 | 2.1 毫秒 | 4.6 毫秒 | 7.9 毫秒 | 16.8 毫秒 |
| 影格之間的時間 | 3.3 毫秒 | 5.3 毫秒 | 9.9 毫秒 | 17.3 毫秒 |

7.7 秒內共 2,182 個影格；只有一次間隔長於 60 Hz 的一個影格。你可以用 `RUSTYFILES_TRACE_LIVE=1` 與 `RUST_LOG=warn,rf_app=info` 自行測量，
並用 `GPUI_FRAME_LOG=frames.tsv` 記錄影格。

#### 已開啟視窗中的新分頁

| 資料夾 | 清單穩定的時間 |
|---|--:|
| 1,000 個檔案 | 約 48 毫秒 |
| 10,000 個檔案 | 約 46 毫秒 |
| 100,000 個檔案 | 約 129 毫秒 |

每多開一個分頁，約需 3 MB 記憶體（1 個分頁：約 32 MB，25 個分頁：約 103 MB）。

#### 依檔名搜尋

rustyfiles 會保存本機磁碟的名稱索引，並從索引回答搜尋。在使用者資料夾的每個子資料夾中搜尋 "rapor"，
該資料夾含有 360 萬個檔案與資料夾，計時至螢幕上的結果不再變動為止：

| | 時間 |
|---|--:|
| rustyfiles 0.4 | **0.38 秒** |
| File Explorer | 1.8 秒（4 次中最快的一次；其餘花了 11–32 秒） |

索引中每個不同的名稱只儲存一次：一個有 480 萬筆項目、但只有 49 萬個不同名稱的磁碟，佔用 61 MB。rustyfiles 執行時，資料夾監看器會讓索引保持最新；
執行之間，NTFS 的變更日誌（以非系統管理員權限讀取）會指出哪些資料夾有變動，因此啟動時只會重新列出這些資料夾：在這裡 C: 磁碟耗時 0.2 秒，而完整走訪需要 30 秒。
應用程式資料、快取、`node_modules` 與 `.git` 不列入索引；若在其中開始搜尋，則會直接走訪磁碟：

| 在 `C:\Windows` 下的 `*.dll`（342,492 個項目，116,851 個資料夾，快取已預熱） | 時間 |
|---|--:|
| rustyfiles | **7.8 秒** |
| `dir /s /b` | 53 秒 |
| PowerShell `Get-ChildItem -Recurse` | 105 秒 |

它在六個背景優先權執行緒上搜尋，因此電腦其餘部分仍保持流暢，而且不會跟隨連接點（junction），這也是它少找到 454 個重複項目的原因。

#### 以 Space 預覽（DirectLook）

| 檔案 | 從按下按鍵到內容就緒 |
|---|--:|
| JPEG，3840 × 2160 | 24–86 毫秒 |
| 原始碼，語法上色 | 23–44 毫秒 |
| 7z 壓縮檔（列出內容） | 19–31 毫秒 |
| MP3（標籤、歌詞） | 27–32 毫秒 |
| H.264 影片，1080p | 第一個影格在 146–185 毫秒內 |

#### 壓縮檔

以 [rarchive](https://github.com/gelenmedya/rarchive) 讀取 RAR，這是我們自己的引擎，對比官方的 UnRAR（解碼並檢查每個 CRC，壓縮檔已在檔案快取中）：

| 壓縮檔 | UnRAR | rarchive |
|---|--:|--:|
| 1.3 GB，32 MB 字典，195 個檔案 | 2.79 秒 | **0.28 秒**（10×） |
| 400 MB 實心（solid） | 0.95 秒 | **0.21 秒**（4.5×） |
| 400 MB，520 個檔案 | 1.03 秒 | **0.063 秒**（16×） |

#### 複製與解壓縮

我們自己的引擎對比 Windows 的（rustyfiles 0.8.6，2026-11-16）：

| | Windows | rustyfiles |
|---|--:|--:|
| 在 100 個資料夾中複製 10,000 個小檔案（41 MB） | 25.6 秒 | **1.8 秒**（14×） |
| 複製一個 1 GB 檔案 | 0.61 秒 | 0.44 秒 |
| 解壓縮一個有 10,000 個檔案的 ZIP | 32.4 秒 | **3.6 秒**（9×） |

複製時會把小檔案分散到多個資料夾，因為同時寫入同一個資料夾的執行緒會彼此等待；單一大檔案的速度受限於磁碟，而非引擎。
它會自行處理名稱衝突（取代、略過或兩者都保留），可以暫停與繼續，能逐位元組驗證每一次複製，也能限制速度。
複製測試腳本為 `crates/rf-win/examples/copybench.rs`。

所有數字：同一台電腦（Ryzen 7 9800X3D、NVMe SSD、Windows 11），2026 年 11 月。

</details>

## 功能概覽

- **瀏覽。** 分頁可拖曳到新視窗、可儲存工作階段、雙窗格、詳細資料、格狀、清單與欄位檢視、分組、預覽窗格、色彩標籤、Shelf、網路共用與 FTP。
- **檔案。** 復原與重做、批次重新命名、按需計算資料夾大小、以樹狀圖顯示磁碟用量、完整的 Explorer 右鍵選單，以及與其他所有應用程式之間的拖放。
- **壓縮檔。** ZIP、7z、RAR、tar 與 ISO 都能像資料夾一樣開啟；ZIP、7z 與 tar 可直接在其中編輯。
- **搜尋。** 你的磁碟名稱索引，以及快速搜尋（<kbd>Alt</kbd>+<kbd>Space</kbd>），可搜尋應用程式、檔案、書籤與 Windows 設定。
- **Windows。** 可選擇設為預設檔案管理員，包括 <kbd>Win</kbd>+<kbd>E</kbd>，以及其他應用程式的開啟與儲存對話框。
- **49 種語言**，跟隨系統語言。

<details>
<summary><b>每項功能的詳細說明</b></summary>

**瀏覽。** 分頁可拖曳重新排序，或拉出成為新視窗。可將視窗的分頁（包括分割的窗格）儲存為具名工作階段，之後再開啟。新分頁會開啟首頁或你目前所在的資料夾（設定 › 一般）。雙窗格。詳細資料、格狀、清單與欄位檢視（清單可像 Explorer 一樣逐欄填滿）、如 Explorer 精簡檢視般的精簡項目、分組、預覽窗格、搜尋、色彩標籤、Shelf、程式庫、網路共用與 FTP。Git 狀態欄。按需計算資料夾大小：點選「計算大小」；結果會在重新啟動後保留，並可顯示磁碟上的大小。在資料夾或磁碟上按「顯示磁碟用量」，會依佔用空間由大到小列出，旁邊是可點擊逐層查看的樹狀圖。
像「程式庫」與「相簿」這類虛擬資料夾會在 rustyfiles 中開啟；Windows 在「本機」下列出的裝置（透過 USB 連接的手機或 Phone Link）會出現在首頁的「裝置」下。Windows 認得的雲端資料夾（OneDrive、Proton Drive、Dropbox、MEGA 等）則位於側欄。

**檔案。** 重新命名、複製、移動、移到資源回收筒的刪除，以及資料夾扁平化，都可以復原與重做。批次重新命名。複製、移動到另一個窗格，或將複製的文字或圖片貼成新檔案。完整的 Explorer 右鍵選單（包括「傳送到」）；可與 Explorer、桌面、瀏覽器與 Outlook 進行拖放；分頁、釘選的資料夾、資料夾與程式都能作為放置目標。使用 [Ferry](#ferry) 將檔案傳送到你的其他電腦，並可用 [HyperDownload](#hyperdownload) 下載。

**壓縮檔。** ZIP、7z、RAR、tar 系列與 ISO 都能像資料夾一樣開啟，包括有密碼保護的 ZIP、7z 與 RAR。可在 ZIP、7z 與 tar 內新增、刪除與重新命名。在每個核心上都能建立 ZIP、7z 與 tar.zst。讀取由 [rarchive](https://github.com/gelenmedya/rarchive) 負責，這是我們自己的純 Rust 引擎（也[登載於 crates.io](https://crates.io/crates/rarchive)）：解碼一個 1.3 GB 的 WinRAR 壓縮檔，速度比 UnRAR 快十倍。

**Windows。** 可選擇設為預設檔案管理員：資料夾、<kbd>Win</kbd>+<kbd>E</kbd>、本機與資源回收筒都會在 rustyfiles 中開啟。也可選擇讓其他應用程式的「開啟」、「儲存」與「選取資料夾」對話框同樣在 rustyfiles 中開啟（設定 › 進階），並且那裡的釘選按鈕可為每個應用程式保留一個起始資料夾。在「相片」或其他 Store 應用程式中開啟的圖片與影片會帶著所在的資料夾，因此其「上一張」與「下一張」可以運作。Explorer 的 `/select,` 與 `shell:` 路徑可以使用。Mica、深色模式與系統強調色。當遊戲或轉譯佔滿 GPU 時，繪圖會切換到 CPU 再切回，不會閃爍。

**位址列中的指令。** 與 Explorer 相同：按 <kbd>Ctrl</kbd>+<kbd>L</kbd>，輸入 `cmd`、`powershell`、`wt` 或 `code .`，然後按 <kbd>Enter</kbd>。文字會先被當作一個位置（路徑、磁碟機、`%VAR%`、`shell:`）；若目前資料夾中有同名的資料夾或檔案，則一律優先。只有在沒有這樣的位置時，才會像「執行」方塊一樣把第一個字當作程式查找，並在目前資料夾中啟動。不會從自動完成的項目啟動任何東西，也不會要求系統管理員權限。設定 › 檔案與資料夾 › 行為 中的開關可將其關閉。

**整理。** 針對資料夾的規則：依名稱、副檔名、類型、大小與時間設定條件，並執行一個動作（移動、移到依範本建立的資料夾、重新命名，或移到資源回收筒）。手動執行時會在預覽中顯示每個步驟，按下「套用」前不會移動任何東西，而且執行可以復原。永遠不會真正刪除任何東西，也不會覆寫任何東西。自動執行的規則預設為關閉。

**搜尋。** 快速搜尋（<kbd>Alt</kbd>+<kbd>Space</kbd>）可找到應用程式、檔案與資料夾、瀏覽器書籤，以及 Windows 的設定頁面。你常開啟的項目會排在前面，而你之前為相同字母所選的項目會成為第一列。

**語言。** 共 49 種，跟隨系統的顯示語言（或在 設定 › 一般 中挑選）。介面以土耳其語與英文撰寫；其他語言以 Files 社群的用語為起點，之後以機器翻譯為主。從右至左的名稱與文字會由右至左顯示（感謝 @ialhabbal）；介面本身尚未鏡像。歡迎提供更正：每種語言都是 `crates/rf-app/locales/` 中的一個 JSON 檔案。

</details>

<img alt="rustyfiles 以格狀檢視顯示一個 Windows 桌布資料夾，其中一張被選取，並在右側的預覽窗格中放大顯示，背景為 HyperGlass" src="docs/readme/grid.png" width="100%">

## DirectLook

選取一個檔案並按下 <kbd>Space</kbd>：影片與音訊有各自的播放器、圖片、文字與程式碼、壓縮檔的內容，以及其他一切的詳細資料卡。方向鍵可在資料夾保持開啟時逐一瀏覽。
它以自己的程式執行，因此沉重的影片不會拖慢檔案管理員；若你開啟它，它也能在 Explorer 與桌面上運作（設定 › DirectLook）。

## Ferry

在同一網路中的其他電腦上，將檔案與資料夾傳送到 rustyfiles，方式類似 AirDrop：按右鍵 › **以 Ferry 傳送**。在 設定 › Ferry 中開啟之前，此功能為關閉狀態。

<img alt="設定 › Ferry，列出兩個附近的裝置：一台已配對的 Windows 電腦及其實測速度，以及一台估計速度的 MacBook；Windows 通知詢問是否接受來自 MacBook 的檔案，狀態列顯示一個傳輸進度為 61%" src="docs/readme/ferry.png" width="100%">

<sub>設定 › Ferry 的繪圖，裝置為虛構，應用程式的用語與版面為原樣。</sub>

<details>
<summary><b>Ferry 如何運作</b></summary>

- 端對端加密（QUIC 搭配 TLS 1.3），從裝置直接到裝置：沒有伺服器、沒有帳號，也沒有任何東西離開你的網路。
- 配對裝置的方式與配對藍牙相同：兩邊都顯示同一組六位數代碼。已配對裝置的檔案可以直接到達而不必詢問；其他一切都會先以通知詢問。
- 你可以決定誰能看到你的電腦：沒有人、你已配對的裝置（預設），或在十分鐘內所有人。開啟 設定 › Ferry 時，所有人都能看見它，讓新裝置能找到它。
- 中斷的傳輸會從停下的地方繼續，每個檔案在取得名稱前都會經過檢查。收到的檔案會被標記為已下載，因此 SmartScreen 會在其中的程式執行前詢問。
- 每個裝置都會顯示估計速度，而 **測試速度** 會在數秒內測量兩個方向的速度。
- macOS 應用程式使用相同的協定，文件記載於 [docs/nearby-protocol.md](docs/nearby-protocol.md)。Windows 防火牆可能會詢問 Ferry：請允許其在私人網路中使用。

</details>

## HyperDownload

rustyfiles 自己的下載管理員，是一個附加元件：設定 › 附加元件 › HyperDownload › **安裝**。可同時以多個連線下載一個檔案，中斷後能從停下的地方續傳，支援種子與磁力連結，
有「下載」頁面，並且影片在下載完成前就能在 DirectLook 中播放。它是一個獨立的程式（[原始碼](https://github.com/gelenmedya/hyperdownload)，MIT），只有在你按下安裝時才會下載，
下載前會先比對 SHA-256 檢查碼，並且可隨時移除。

## Windows 實驗功能：HyperGlass、Dock 與桌面

預設關閉，且仍處於早期階段：僅在一台配有雙螢幕的電腦上建置與測試。預期會有粗糙的地方，歡迎回報你的發現。Explorer 仍在背後執行；Windows 的任何東西都不會在磁碟上被移除或取代。

<img alt="一個桌面，底部有 rustyfiles Dock（三個島狀區塊：開始與搜尋、釘選與執行中的應用程式、系統匣與時鐘），左側開啟了 Dock 的開始面板，顯示釘選的應用程式與最近的檔案；中間是 HyperSearch，顯示搜尋 adobe 找到的應用程式；右側是控制中心，有 Wi-Fi、藍牙、乙太網路、正在播放的歌曲、音量與亮度" src="docs/readme/shell.png" width="100%">

<sub>開始、HyperSearch 與控制中心，由同一桌面的三張截圖拼成（它們不會同時開啟）。</sub>

<details>
<summary><b>每個部分的功能</b></summary>

- **HyperGlass**（設定 › 外觀）是第五種背景：視窗的背景是玻璃，會折射並使後方的內容起霧。
- **Dock**（設定 › Dock，一個開關）會在每個螢幕底部放置三個玻璃島狀區塊，取代 Windows 的工作列：各自的開始面板與搜尋、執行中與釘選的應用程式（附視窗預覽）、系統匣圖示、控制中心與通知面板。開啟時，輕按 Windows 鍵會開啟 Dock 的開始面板，<kbd>Alt</kbd>+<kbd>Tab</kbd> 則會開啟它的切換器。關閉 Dock 後，Windows 原本的就會回來。
- **桌面**由 rustyfiles 繪製，隨 Dock 一同提供：你的桌布與圖示，並可選擇圓角。
- **每個螢幕各自的亮度。** Windows 只有筆電自身螢幕的滑桿；Dock 的控制中心則為每個螢幕都提供一個。外接螢幕透過 DDC/CI 設定，過去十五年來幾乎所有桌上型螢幕都能透過 DisplayPort、HDMI 與 DVI 支援它。若某螢幕沒有滑桿，請在螢幕自己的選單中確認 DDC/CI 已開啟；有些擴充座、KVM 切換器與轉接器不會傳遞它。

</details>

## macOS（測試版）

同一個檔案管理員，以 AppKit 與 Liquid Glass 原生繪製，架構於同一個 Rust 核心之上：分頁、雙窗格、所有檢視、以 <kbd>Space</kbd> 使用 Quick Look、壓縮檔如資料夾、批次重新命名、磁碟用量、
Shelf、色彩標籤、Git 狀態、連線到伺服器、Ferry，以及相同的 49 種語言。Dock、桌面、HyperSearch 與 DirectLook 目前仍留在 Windows 上，而 HyperDownload 的 Mac 版本將在其自己的版本中推出。
支援 Apple 晶片與 Intel 晶片的 Mac，需要 macOS 26（Tahoe）。它會像 Windows 版一樣自行更新。

## 安裝

**Windows。** [下載 rustyfiles-win-Setup.exe](https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-win-Setup.exe)
（約 50 MB）並開啟。若 Windows 顯示「Windows 已保護你的電腦」，請按一下 **其他資訊**，再按 **仍要執行**：安裝程式尚未經過程式碼簽章，而每個版本都會在
[VirusTotal](https://www.virustotal.com) 掃描（報告連結列在其發行說明結尾）。
rustyfiles 會自動開啟，並出現在你的開始功能表中。不需要系統管理員密碼；更新會在背景安靜地安裝。

**macOS（測試版）。** [下載 rustyfiles-mac.dmg](https://github.com/gelenmedya/rustyfiles/releases/latest/download/rustyfiles-mac.dmg)，
開啟後將 rustyfiles 拖到「應用程式」。它已由 Apple 簽署並完成公證，因此與其他 Mac 應用程式一樣可以直接開啟。

<details>
<summary><b>Scoop、可攜式、ARM64、預設檔案管理員、解除安裝</b></summary>

**使用 [Scoop](https://scoop.sh)：**

```powershell
scoop bucket add rustyfiles https://github.com/gelenmedya/scoop-rustyfiles
scoop install rustyfiles/rustyfiles
```

**可攜式或 ARM64：** 可攜式 zip 與 ARM64 安裝程式（`rustyfiles-win-arm64-Setup.exe`）都位於 [Releases](https://github.com/gelenmedya/rustyfiles/releases) 頁面。
需要 Windows 11（Windows 10 未經測試）。Linux 已列入規劃。

**在 rustyfiles 中開啟資料夾與 <kbd>Win</kbd>+<kbd>E</kbd>：** 開啟 設定 › 進階 › 設為預設檔案管理員。你隨時都可以關閉它。

**解除安裝**：如同任何應用程式，從 Windows 設定 › 應用程式 進行。rustyfiles 所做的所有變更都會恢復為 File Explorer。

**其他應用程式的開啟／儲存對話框失敗或當機**，是否在 rustyfiles 的檔案被移除且未經解除安裝之後（例如病毒掃描程式或更新失敗）？再次啟動 rustyfiles 即可修復。若它已無法啟動，這段 PowerShell 指令會讓每個應用程式都恢復 Windows 自己的對話框：

```powershell
Remove-Item -Recurse "HKCU:\Software\Classes\CLSID\{DC1C5A9C-E88A-4dde-A5A1-60F82A20AEF7}", "HKCU:\Software\Classes\CLSID\{C0B4E2F3-BA21-4773-8DBA-335EC946EB8B}"
```

</details>

<details>
<summary><b>從原始碼建置</b></summary>

你需要 1.95 或更新版本的 [Rust](https://rustup.rs)，以及安裝了「使用 C++ 的桌面開發」工作負載的 Visual Studio Build Tools
（一個相依套件 zstd 會編譯 C 程式碼）。

```powershell
git clone https://github.com/gelenmedya/rustyfiles
cd rustyfiles
.\scripts\install.ps1            # 建置，然後為你的使用者安裝
.\scripts\install.ps1 -Default   # 同時設為預設檔案管理員
.\scripts\install.ps1 -Uninstall
```

`cargo run --release -p rf-app` 可在不安裝的情況下啟動它。macOS 應用程式位於 `macos/` 與 `crates/rf-mac`，不在 Windows 工作區之中。

</details>

**有問題或點子？** 歡迎來 [Discord](https://discord.gg/K2banZ36zJ) 打聲招呼，或造訪 [rustyfiles.com](https://rustyfiles.com)。

## 內部結構

<details>
<summary><b>鍵盤</b></summary>

| 按鍵 | 動作 |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>T</kbd> / <kbd>Ctrl</kbd>+<kbd>W</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd> | 新分頁、關閉分頁、重新開啟已關閉的分頁 |
| <kbd>Ctrl</kbd>+<kbd>L</kbd> 或 <kbd>Alt</kbd>+<kbd>D</kbd> | 位址列 |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> | 搜尋（<kbd>Enter</kbd> 也會搜尋子資料夾） |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> 或 <kbd>F1</kbd> | 命令選擇區 |
| <kbd>Ctrl</kbd>+<kbd>P</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>A</kbd> | 尋找你造訪過的資料夾，尋找分頁 |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>1</kbd> … <kbd>5</kbd> | 詳細資料、格狀、欄位、卡片、清單 |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd>、<kbd>F6</kbd> | 雙窗格，切換窗格 |
| <kbd>Shift</kbd>+<kbd>F5</kbd> / <kbd>Shift</kbd>+<kbd>F6</kbd> | 複製、移到另一個窗格 |
| <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>D</kbd> | 複製一份 |
| <kbd>Alt</kbd>+<kbd>P</kbd>、<kbd>Ctrl</kbd>+<kbd>B</kbd> | 預覽窗格，側欄 |
| <kbd>Space</kbd> | DirectLook |
| <kbd>F2</kbd> | 重新命名（選取多個項目時為批次重新命名） |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> / <kbd>Ctrl</kbd>+<kbd>Y</kbd> | 復原、重做 |
| <kbd>Ctrl</kbd>+<kbd>H</kbd> | 顯示隱藏項目 |

每個快速鍵都可以在 設定 › 動作 中更改：點選一個命令，然後按下新的按鍵。

</details>

<details>
<summary><b>專案結構</b></summary>

| 套件 | 內容 |
|---|---|
| `crates/rf-app` | Windows 介面，以及它的兩個程式：`rustyfiles` 與 `directlook`（`src/look/`） |
| `crates/rf-win` | Windows 與 Shell 整合，檔案操作引擎 |
| `crates/rf-core` | 兩個平台共用的模型：位置、歷史、項目、格式 |
| `crates/rf-archive` | 壓縮檔操作：瀏覽、解壓縮、建立、編輯 |
| `crates/rf-nearby` | Ferry，與 macOS 應用程式共用 |
| `crates/rf-player` | DirectLook 的影片與音訊播放器 |
| `crates/rustdecoder` | 我們自己的影片引擎，仍在開發中：MP4/MOV 與 MKV/WebM；H.264、HEVC、VP9 與 AV1 透過 D3D11 Video 在顯示卡上解碼，在其測試中與 ffmpeg / dav1d 逐位元一致；ProRes 與 DNxHR 在 CPU 上處理；音訊與 A/V 同步（[設計](docs/design/rustdecoder.md)） |
| `macos/`、`crates/rf-mac` | macOS 應用程式（AppKit）及其與 Rust 核心的橋接 |

Windows 介面以 [GPUI](https://gpui.rs) 建構，這是 Zed 編輯器背後的框架。它的 Windows 層是一個分支，加入了 GPU／CPU 切換功能：
[gelenmedya/gpui-pre-windows](https://github.com/gelenmedya/gpui-pre-windows)。

</details>

<details>
<summary><b>環境變數</b></summary>

| 變數 | 效果 |
|---|---|
| `RUSTYFILES_RENDERER` = `gpu`、`cpu` 或 `auto` | 強制指定視窗在哪裡繪製 |
| `RUSTYFILES_CONFIG_DIR` = 資料夾 | 將設定保存在另一個資料夾（可攜式使用） |
| `RUSTYFILES_MULTI_INSTANCE=1` | 允許同時執行多個副本（測試用） |
| `RUSTYFILES_FRAME_LOG=1` | 記錄影格時間 |
| `RUSTYFILES_LOOK_TRACE=1` | 將 DirectLook 的計時記錄到 stderr |
| `RUSTYFILES_MUTE=1` | DirectLook 靜音播放（測試用） |

設定儲存於 `%APPDATA%\rustyfiles\settings.toml`，DirectLook 的設定則在其旁的 `directlook.toml` 中。

</details>

<details>
<summary><b>它是如何製作的</b></summary>

rustyfiles 是以 AI 程式設計工具撰寫的，這並不是秘密。一位開發者決定要建置什麼、它應該如何運作，在真實的電腦上（雙螢幕、日常工作）試用每一項變更，並淘汰站不住腳的部分。
AI 寫了大部分的程式碼；但決定什麼能出貨的，不是它。

確保這一點誠實的做法：

- **測試。** 每次變更推送之前，會執行超過 4,000 項自動化測試。
- **測量。** 速度數字來自應用程式內建的測試（設定 › 速度測試），因此任何人都能在自己的機器上驗證。
- **回報。** 錯誤來自使用它的人。每個議題都會得到回覆，修正會公開進行，而回報的人在結案前會被請求試用。
- **程式碼。** 全部都在這裡，依 MIT 授權，供你閱讀與評斷。

為何選擇 Rust：檔案管理員必須在資料夾有 100,000 個檔案時依然快速，而且絕不能遺失任何人的資料。Rust 在沒有垃圾回收器的情況下提供速度，並在出貨之前排除了整類的當機。
rustyfiles 是從頭撰寫的全新應用程式，而不是 Explorer 的重寫；它與 Explorer 並存，並且不會更動磁碟上的任何 Windows 內容。

</details>

## 狀態

rustyfiles 還很年輕，它的作者每天都用它取代 Explorer。macOS 應用程式是同一核心上的第一個測試版，之後將推出 Linux。歡迎回報錯誤與提交拉取請求。

## 收錄於

- [AlternativeTo](https://alternativeto.net/software/rustyfiles/about/)，作為 File Explorer 的替代品
- [Awesome GPUI](https://github.com/zed-industries/awesome-gpui#productivity)，列於 Productivity 類別
- [Awesome Rust](https://github.com/rust-unofficial/awesome-rust#file-manager)，列於 File manager 類別
- [Scoop](https://scoop.sh)，來自其自己的 bucket（見 [安裝](#安裝)）

## 致謝

rustyfiles 建立在其他人的成果之上，一路上借鑑了許多想法。

[Files](https://github.com/files-community/Files) 展示了現代 Windows 檔案管理員可以是什麼樣子。它的功能清單是我們的核對表，它的設定與版面塑造了我們的，而它以 Files 取代 Explorer 作為預設檔案管理員的做法，也引導了我們。感謝 Files 社群。

[GPUI](https://gpui.rs)，即 [Zed](https://github.com/zed-industries/zed) 編輯器的 UI 框架，在 Windows 上以 GPU 繪製 rustyfiles 的每一個像素，並讓如此快速的視窗成為可能。感謝 Zed 團隊的建置與分享，也感謝 [Longbridge](https://github.com/longbridge/gpui-kit) 提供 gpui-kit 這套我們所依賴的元件。

DirectLook 承襲 macOS Quick Look 的理念，而 [QuickLook](https://github.com/QL-Win/QuickLook)（由 Paddy Xu 製作）率先將它帶到 Windows。rarchive 的 RAR 格式是從 Alexander Roshal 的 UnRAR 原始碼中學來的。

以 [windows-rs](https://github.com/microsoft/windows-rs)、[Velopack](https://velopack.io)（安裝與更新）、[Lucide](https://lucide.dev) 圖示、[zip](https://github.com/zip-rs/zip2)、
[sevenz-rust2](https://github.com/hasenbanck/sevenz-rust)、[tar](https://github.com/alexcrichton/tar-rs)、
[notify](https://github.com/notify-rs/notify) 等眾多套件建構而成。圖片使用了 Atelier Triay 的
[Bricolage Grotesque](https://github.com/ateliertriay/bricolage)、
[Instrument Sans](https://fonts.google.com/specimen/Instrument+Sans) 與 [Doto](https://fonts.google.com/specimen/Doto)。

## 授權

MIT，見 [LICENSE](LICENSE)。GPUI 分支在其自己的儲存庫中保留 Zed 的 Apache-2.0 授權。
