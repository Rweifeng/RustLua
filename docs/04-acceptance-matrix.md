# 楠屾敹鐭╅樀

## 浣跨敤璇存槑

鏈煩闃佃褰曟瘡涓换鍔℃垨閲岀▼纰戠殑鍙獙璇佺粨鏋溿€俙PASS` 琛ㄧず宸插畬鎴愰獙鏀讹紝`BLOCKED` 琛ㄧず瀹炵幇宸插噯澶囦絾鍙楀閮ㄦ潯浠堕樆濉烇紝`PENDING` 琛ㄧず灏氭湭寮€濮嬶紝`FAIL` 琛ㄧず楠屾敹澶辫触銆?

| ID | 鍔熻兘 | 鍓嶇疆鏉′欢 | 鎿嶄綔/鍛戒护 | 棰勬湡缁撴灉 | 鑷姩鍖栨祴璇?| 鎵嬪伐璇佹嵁 | 鐘舵€?| 鐗堟湰 |
|---|---|---|---|---|---|---|---|---|
| M0-A01 | 椤圭洰缁撴瀯 | M0-01 | 妫€鏌ョ洰褰?| 鎵€鏈夊熀绾跨洰褰曞瓨鍦?| 鐩綍妫€鏌ヨ剼鏈?| `artifacts/screenshots/M0/` | PASS | M0 |
| M0-A02 | 椤圭洰绔犵▼ | M0-01 | 闃呰绔犵▼ | 鐩爣銆佽寖鍥淬€侀噷绋嬬鍜岄獙鏀堕棬瀹屾暣 | 鏂囨。妫€鏌?| 绔犵▼鏂囦欢 | PASS | M0 |
| M0-A03 | 浠诲姟妯℃澘 | M0-01 | 鍒涘缓绀轰緥浠诲姟 | 浠诲姟鍚洰鏍囥€佽寖鍥淬€佹祴璇曞拰楠屾敹 | 妯℃澘妫€鏌?| 绀轰緥浠诲姟鏂囦欢 | PASS | M0 |
| M0-A04 | 鍙户缁紑鍙?| M0-01 | 鏂?AI 浼氳瘽璇诲彇鏂囨。 | 鑳界悊瑙ｅ綋鍓嶇姸鎬佸拰涓嬩竴姝?| 鏂囨。妫€鏌?| 浼氳瘽楠屾敹璁板綍 | PASS | M0 |
| M0-A05 | 璐ㄩ噺闂ㄨ剼鏈?| M0-03 | `./scripts/verify.ps1` | 鎵ц metadata銆乫mt銆乧lippy銆乼est銆乥uild | PowerShell 鑴氭湰 | `artifacts/logs/quality/verify-20260820-113948.log` | PASS | M0 |
| M0-A06 | 娴嬭瘯鍏ュ彛 | M0-03 | `./scripts/test.ps1` | 鎵ц娴嬭瘯璐ㄩ噺闂?| PowerShell 鑴氭湰 | `artifacts/logs/quality/verify-20260820-113952.log` | PASS | M0 |
| M0-A07 | Release 鏋勫缓鍏ュ彛 | M0-03 | `./scripts/build-release.ps1` | 鐢熸垚 Release 杩愯鍣?| PowerShell 鑴氭湰 | `artifacts/builds/release/game_runner.exe` | PASS | M0 |
| M0-A08 | CI 璐ㄩ噺闂?| M0-03 | GitHub Actions Windows runner | format銆乧lippy銆乼est銆丏ebug/Release build 閫氳繃 | CI workflow | Actions 璁板綍 | PENDING | M0 |
| M0-A09 | M0-04 鏂囦欢瀹屾暣鎬?| M0-04 | 妫€鏌ラ獙鏀剁煩闃靛拰椋庨櫓鐧昏 | 涓や唤鏂囨。 UTF-8 姝ｅ父銆佸瓧娈靛畬鏁淬€佸彲鐩存帴浣跨敤 | 鏂囨。妫€鏌?| 鏂囦欢鍐呭 | PASS | M0 |
| M0-A10 | 椋庨櫓鐧昏 | M0-04 | 妫€鏌ラ闄╄〃 | 椋庨櫓鏈夋鐜囥€佸奖鍝嶃€佸簲瀵圭瓥鐣ャ€佽礋璐ｄ汉鍜岀姸鎬?| 鏂囨。妫€鏌?| 椋庨櫓鐧昏琛?| PASS | M0 |
| M0-A11 | M0 閲岀▼纰戦獙鏀?| M0-01 鑷?M0-04 | 鎵ц瀹屾暣鍥炲綊楠屾敹 | 鏈湴璐ㄩ噺闂ㄣ€佹瀯寤恒€丏emo銆佹枃妗ｅ拰椋庨櫓妫€鏌ラ€氳繃 | 鍥炲綊鍛戒护闆?| `tasks/done/M0-05-M0閲岀▼纰戦獙鏀?md` | PASS | M0 |
| M0-A12 | M0-06 鍩虹嚎鏋勫缓鍜屾爣绛?| M0-01 鑷?M0-05 | 鎵ц璐ㄩ噺闂ㄥ苟妫€鏌?Git 鏍囩 | 鍩虹嚎鎻愪氦鍙瀯寤轰笖瀛樺湪 `v0.0.1-baseline` | 璐ㄩ噺闂ㄥ拰 Git 妫€鏌?| `tasks/done/M0-06-棣栨鍩虹嚎鏋勫缓鍜岀増鏈爣绛?md` | PASS | M0 |

## M0-04/M0-05 楠屾敹缁撹

- 楠屾敹鐭╅樀宸茶鐩?M0-01銆丮0-03銆丮0-04 鍜?M0-05 鐨勫綋鍓嶇姸鎬侊紱
- 宸插尯鍒嗛€氳繃銆侀樆濉炪€佸緟澶勭悊涓夌鐘舵€侊紱
- M0-02/M0-03 鐨?Rust 鍛戒护绾ч獙璇佸凡瀹屾垚锛岀粨鏋滃凡鍥炲～鍒颁换鍔¤褰曞拰鏈煩闃碉紱
- M0-04 鏂囦欢瀹屾暣鎬у拰椋庨櫓鐧昏楠屾敹閫氳繃锛?
- M0-05 鏈湴閲岀▼纰戝洖褰掗獙鏀堕€氳繃锛?
- M0-06 鍩虹嚎鏋勫缓鍜?`v0.0.1-baseline` 鏍囩楠屾敹閫氳繃锛岄」鐩斁琛岃繘鍏?M1锛?
- M0-A08 GitHub Actions 杩滅▼鎵ц浠嶄负 `PENDING`锛屼笉褰卞搷鏈湴 M0 鏀捐銆?
| M1-A01 | 绐楀彛涓庡簲鐢ㄧ敓鍛藉懆鏈?| M0 | `./scripts/verify.ps1`銆佽繍琛?`game_runner` | 绐楀彛鍒涘缓銆佸叧闂€佺缉鏀俱€佹渶灏忓寲/鎭㈠涓嶅穿婧冿紝鐢熷懡鍛ㄦ湡鏃ュ織椤哄簭姝ｇ‘ | 鐘舵€佹満鍗曞厓娴嬭瘯 | `artifacts/logs/M1-01-window-session.stdout.log`銆乣artifacts/logs/M1-01-window-interaction.stdout.log` | PASS | M1 |
| M1-A02 | wgpu 鍒濆鍖栦笌娓呭睆 | M1-A01 | `./scripts/verify.ps1`銆佽繍琛?GPU Demo 骞惰皟鏁寸獥鍙ｅ昂瀵?| Debug/Release 鍧囧垵濮嬪寲 GPU銆佹竻灞忋€佺缉鏀惧悗缁х画娓叉煋锛岄敊璇垎鏀湁澶勭悊 | workspace 娴嬭瘯涓?Clippy | `artifacts/logs/M1-02-wgpu-session.stdout.log` | PASS | M1 |
| M1-A03 | Lua VM 妗ユ帴 | M1-A02 | `cargo test --workspace`銆乣./scripts/verify.ps1`銆佽繍琛?Lua Demo | Lua 璋?Rust API銆丷ust 璋?Lua 鍑芥暟銆佽剼鏈矾寰?琛屽彿閿欒銆佽繍琛屾椂閿欒鏃ュ織銆丏ebug/Release 鏋勫缓 | `engine_core` 鍗曞厓娴嬭瘯 | `artifacts/logs/M1-03-lua-session.stdout.log`銆乣artifacts/logs/quality/verify-20260820-144431.log`銆乣artifacts/logs/quality/verify-20260820-144146.log` | PASS | M1 |
| M1-A04 | 鏃ュ織涓庨敊璇郴缁?| M1-A03 | `cargo test --workspace`锛沗./scripts/verify.ps1`锛沗./scripts/verify.ps1 -Release`锛涜繍琛岀湡瀹?Debug Demo | 鎺у埗鍙板拰鏂囦欢鏃ュ織鍖呭惈鏃堕棿銆佺骇鍒€乼arget锛涙敮鎸佹ā鍧楄繃婊わ紱Lua/娓叉煋閿欒鍖呭惈涓婁笅鏂囧拰瑙ｅ喅寤鸿 | logging 鍗曞厓娴嬭瘯锛涘伐浣滃尯璐ㄩ噺闂?| `engine/artifacts/logs/M1-04-logging-session-final.stdout.log`锛沗engine/artifacts/logs/game-runner.log`锛沗engine/artifacts/logs/quality/verify-20260820-151137.log`锛沗engine/artifacts/logs/quality/verify-20260820-151819.log` | PASS | M1 |
| M1-A05 | 鍥哄畾鏃堕棿姝?| M1-A04 | `cargo test --workspace`銆乣./scripts/verify.ps1`銆乣./scripts/verify.ps1 -Release`銆佽繍琛?Debug/Release Demo | 60Hz 鍥哄畾 dt銆佹渶澶ц拷璧舵鏁般€侀槻姝㈣灪鏃嬫浜°€佹殏鍋滄仮澶嶄笉閲嶆斁鏃ф椂闂达紝杈撳嚭 dt/accumulator/姝ユ暟缁熻 | `time_step` 鍗曞厓娴嬭瘯锛涘伐浣滃尯璐ㄩ噺闂?| `artifacts/logs/M1-05-fixed-timestep-session-final.stdout.log`锛沗artifacts/logs/M1-05-fixed-timestep-session-release.stdout.log`锛沗artifacts/logs/quality/verify-20260820-160225.log`锛沗artifacts/logs/quality/verify-20260820-160242.log` | PASS | M1 |
| M1-A06 | 杈撳叆蹇収 | M1-A05 | `cargo test --workspace`銆乣./scripts/verify.ps1`銆乣./scripts/verify.ps1 -Release`銆佽繍琛?Debug/Release Demo | 閿洏/榧犳爣/鍏夋爣浜嬩欢杩涘叆蹇収锛涙敮鎸佹寜涓嬨€佹寔缁€侀噴鏀撅紱WASD 杞村€硷紱鐒︾偣涓㈠け鍜屾殏鍋滀笉浜х敓鍗￠敭锛汱ua 鍙鍙栬緭鍏?API | `input` 鍗曞厓娴嬭瘯锛汱ua 妗ユ帴娴嬭瘯锛涘伐浣滃尯璐ㄩ噺闂?| `artifacts/logs/M1-06-input-snapshot-session-debug.stdout.log`锛沗artifacts/logs/M1-06-input-snapshot-session-release.stdout.log`锛沗artifacts/logs/quality/verify-20260820-162504.log`锛沗artifacts/logs/quality/verify-20260820-162526.log` | PASS | M1 |
| M1-A07 | ????? Debug overlay | M1-A06 | `cargo test --workspace`?`./scripts/verify.ps1`?`./scripts/verify.ps1 -Release`??? Debug/Release Demo | ?? FPS?Frame ms?Update ms?Lua ms ???????????? overlay?F3 ????Release ??? | `performance` ????????????????/GPU Demo | `artifacts/logs/M1-07-overlay-session-debug.stdout.log`?`artifacts/logs/M1-07-overlay-session-debug.stderr.log`?`artifacts/logs/M1-07-overlay-session-release.stdout.log`?`artifacts/logs/M1-07-overlay-session-release.stderr.log`?`artifacts/logs/quality/verify-20260820-164230.log`?`artifacts/logs/quality/verify-20260820-164231.log` | PASS | M1 |
| M1-A08 | M1 鍩虹杩愯鏃堕噷绋嬬楠屾敹 | M1-A01 鑷?M1-A07 | `./scripts/verify.ps1 -KeepLog`銆乣./scripts/verify.ps1 -Release -KeepLog`銆佽繍琛?Debug/Release Demo銆佹鏌?Git 鍘嗗彶 | M1-01 鑷?M1-07 鍏ㄩ儴 PASS锛汥ebug/Release 璐ㄩ噺闂ㄩ€氳繃锛涚湡瀹炵獥鍙ｃ€丟PU銆丩ua銆佸浐瀹氭椂闂存銆佽緭鍏ュ揩鐓у拰鎬ц兘 overlay 鍙繍琛岋紱鎻愪氦鍙拷婧苟鍒涘缓 `v0.1-foundation` 鏍囩 | M1 浠诲姟璁板綍銆佸伐浣滃尯娴嬭瘯銆丆lippy銆丏ebug/Release 鏋勫缓銆佺湡瀹?Demo | `artifacts/logs/quality/verify-20260820-170201.log`锛沗artifacts/logs/quality/verify-20260820-170204.log`锛沗artifacts/logs/M1-foundation-session-debug.stdout.log`锛沗artifacts/logs/M1-foundation-session-debug.stderr.log`锛沗artifacts/logs/M1-foundation-session-release.stdout.log`锛沗artifacts/logs/M1-foundation-session-release.stderr.log` | PASS | M1 |
| M2-A01 | EntityId 涓?generation 鍙ユ焺 | M1 | `cargo test -p engine_core --all-features`銆乣./scripts/verify.ps1 -KeepLog`銆乣./scripts/verify.ps1 -Release -KeepLog` | 鍒犻櫎瀹炰綋鍚庢棫鍙ユ焺澶辨晥锛泂lot 澶嶇敤鐢熸垚鏂?generation锛涙棫鍙ユ焺涓嶈兘璇搷浣滄柊瀹炰綋锛汥ebug/Release 璐ㄩ噺闂ㄩ€氳繃 | `entity` 鍗曞厓娴嬭瘯锛泈orkspace Clippy銆佹祴璇曞拰鏋勫缓 | `tasks/done/M2-01-EntityId.md`锛沗artifacts/logs/quality/verify-20260820-170812.log`锛沗artifacts/logs/quality/verify-20260820-170836.log` | PASS | M2 |
| M2-A02 | World 涓庢渶灏忕粍浠跺瓨鍌?| M2-A01 | `cargo test -p engine_core --all-features`銆乣./scripts/verify.ps1 -KeepLog`銆乣./scripts/verify.ps1 -Release -KeepLog` | 鍒涘缓/閿€姣佸疄浣擄紱缁勪欢澧炲垹鏀规煡锛涘崟缁勪欢涓庡弻缁勪欢缁勫悎鏌ヨ锛涘垹闄ゅ疄浣撴竻鐞嗙粍浠讹紱鏃犳晥鍙ユ焺鎷掔粷璁块棶 | world 鍗曞厓娴嬭瘯銆丆lippy銆丏ebug/Release 璐ㄩ噺闂?| `tasks/done/M2-02-World鍜屾渶灏忕粍浠跺瓨鍌?md`锛沗artifacts/logs/quality/verify-20260820-171636.log`锛沗artifacts/logs/quality/verify-20260820-171647.log` | PASS | M2 |
| M2-A03 | Transform 涓?Velocity 绯荤粺 | M2-A02 | `cargo test -p engine_core --all-features`銆乣./scripts/verify.ps1 -KeepLog`銆乣./scripts/verify.ps1 -Release -KeepLog` | Transform 榛樿鍊硷紱绾块€熷害鏇存柊浣嶇疆锛涜閫熷害鏇存柊鏃嬭浆锛涚己澶辩粍浠跺畨鍏ㄨ烦杩囷紱闈炴硶 dt 鎷掔粷锛涢攢姣佸疄浣撲笉鍙備笌鏇存柊 | transform 鍗曞厓娴嬭瘯銆丆lippy銆丏ebug/Release 璐ㄩ噺闂?| `tasks/done/M2-03-Transform鍜孷elocity绯荤粺.md`锛沗artifacts/logs/quality/verify-20260820-172853.log`锛沗artifacts/logs/quality/verify-20260820-172857.log` | PASS | M2 |
| M2-A04 | 绯荤粺璋冨害鍣?| M2-A03 | `cargo test -p engine_core --all-features`銆乣./scripts/verify.ps1 -KeepLog`銆乣./scripts/verify.ps1 -Release -KeepLog` | 闃舵椤哄簭锛涗緷璧栨嫇鎵戯紱缂哄け/閲嶅/闈炴硶渚濊禆鎷掔粷锛涘惊鐜緷璧栨姤閿欙紱绯荤粺閿欒鍜?panic 鍖呭惈妯″潡涓婁笅鏂囷紱绯荤粺鑰楁椂璁板綍 | scheduler 鍗曞厓娴嬭瘯銆丆lippy銆丏ebug/Release 璐ㄩ噺闂?| `tasks/done/M2-04-绯荤粺璋冨害鍣?md`锛沗artifacts/logs/quality/verify-20260820-174941.log`锛沗artifacts/logs/quality/verify-20260820-174947.log` | PASS | M2 |
| M2-A05 | 浜嬩欢闃熷垪 | M2-A04 | `cargo test -p engine_core --all-features`銆乣./scripts/verify.ps1 -KeepLog`銆乣./scripts/verify.ps1 -Release -KeepLog` | 绫诲瀷鍖栦簨浠讹紱鍙戝竷椤哄簭锛涘抚杈圭晫闅旂锛涘崟娆℃秷璐癸紱鏈秷璐逛簨浠舵竻鐞嗭紱褰撳墠甯ф竻绌轰笉褰卞搷寰呭彂甯冧簨浠?| events 鍗曞厓娴嬭瘯銆丆lippy銆丏ebug/Release 璐ㄩ噺闂?| `tasks/done/M2-05-浜嬩欢闃熷垪.md`锛沗artifacts/logs/quality/verify-20260820-175736.log`锛沗artifacts/logs/quality/verify-20260820-175741.log` | PASS | M2 |
| M2-A06 | Command 队列 | M2-A05 | `cargo test -p engine_core --all-features`、`./scripts/verify.ps1 -KeepLog`、`./scripts/verify.ps1 -Release -KeepLog` | 延迟 spawn/despawn；组件变更；命令顺序；批次只应用一次；错误包含命令上下文 | commands 单元测试、Clippy、Debug/Release 质量门 | `tasks/done/M2-06-Command队列.md`、`artifacts/logs/quality/verify-*.log` | PASS | M2 |
