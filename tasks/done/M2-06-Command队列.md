# M2-06 Command 闃熷垪

## 鐘舵€?
- 鐘舵€侊細PASS
- 瀹屾垚鏃ユ湡锛?026-08-20
- Git 鎻愪氦锛氬緟鎻愪氦

## 鐩爣

鍦?ECS 绯荤粺杩唬鏈熼棿锛屽皢瀹炰綋鐢熷懡鍛ㄦ湡鍜岀粍浠跺彉鏇村欢杩熷埌鏄惧紡 `apply` 闃舵鎵ц锛岄伩鍏嶇郴缁熸寔鏈夎凯浠ｆ暟鎹椂鐩存帴淇敼 `World`锛屽苟淇濊瘉鍛戒护鎸?FIFO 椤哄簭銆佹瘡鎵瑰彧搴旂敤涓€娆°€?
## 瀹炴柦鍐呭

- 鏂板 `engine_core::commands::CommandQueue`锛?- 鏀寔 `spawn`銆乣despawn`銆乣insert`銆乣remove` 鍛戒护锛?- 浣跨敤 `SpawnToken` 寮曠敤鍚屼竴鎵规涓皻鏈垱寤虹殑瀹炰綋锛?- 鏀寔 `spawn -> insert`銆乣spawn -> despawn` 绛夊悓鎵规鍛戒护閾撅紱
- 閫氳繃 generation 瀹夊叏鐨?`EntityId` 闃叉杩囨湡鍙ユ焺璇搷浣滃鐢ㄦЫ浣嶏紱
- `apply` 鍦ㄦ墽琛屽墠鍙栬蛋褰撳墠鎵规锛屽け璐ュ悗涓嶄細閲嶅鎵ц宸叉秷璐圭殑鍛戒护锛?- 閿欒鍖呭惈鍛戒护绱㈠紩銆佹搷浣滃悕绉板拰搴曞眰 World 閿欒銆?
## 涓昏 API

```rust
CommandQueue::new()
CommandQueue::spawn() -> SpawnToken
CommandQueue::despawn(target)
CommandQueue::insert::<T>(target, component)
CommandQueue::remove::<T>(target)
CommandQueue::len()
CommandQueue::is_empty()
CommandQueue::apply(&mut world)
```

鐩爣鍙互鏄凡鏈?`EntityId`锛屼篃鍙互鏄湰鎵规 `spawn()` 杩斿洖鐨?`SpawnToken`銆?
## 楠屾敹缁撴灉

| 楠屾敹椤?| 缁撴灉 | 鑷姩鍖栬瘉鎹?|
|---|---|---|
| 绯荤粺杩唬鏈熼棿閿€姣佽姹備笉绔嬪嵆淇敼 World | PASS | `commands::tests::despawn_during_iteration_is_deferred` |
| 鍛戒护鎸夊叆闃熼『搴忔墽琛?| PASS | `commands::tests::queues_mutations_until_apply_and_preserves_order` |
| 鍚屾壒娆?`spawn -> insert` 鍙В鏋愪复鏃跺彞鏌?| PASS | `commands::tests::spawn_token_can_be_used_by_later_commands_in_same_batch` |
| 鍚屾壒娆?`spawn -> despawn` 瀹夊叏鎵ц | PASS | `commands::tests::spawn_then_despawn_is_applied_safely` |
| 杩囨湡 EntityId 涓嶄細璇搷浣滄柊瀹炰綋 | PASS | `commands::tests::stale_existing_targets_fail_without_touching_reused_entities` |
| 闈炴硶涓存椂鍙ユ焺杩斿洖鏄庣‘涓婁笅鏂囬敊璇?| PASS | `commands::tests::missing_token_reports_context_and_batch_is_consumed` |
| 澶辫触鎵规涓嶄細琚噸澶嶆墽琛?| PASS | `commands::tests::missing_token_reports_context_and_batch_is_consumed` |
| 缁勪欢绉婚櫎鍛戒护鍙敤 | PASS | `commands::tests::invalid_entity_id_is_not_accepted_as_another_entity` |
| engine_core 娴嬭瘯 | PASS | `cargo test -p engine_core --all-features`锛?3 椤归€氳繃 |
| Debug/Release 璐ㄩ噺闂?| 寰呰繍琛?| 瑙佹湰浠诲姟鏈€缁堥獙鏀惰褰?|

## 璁捐杈圭晫

褰撳墠瀹炵幇鏄崟绾跨▼銆佹樉寮忔壒澶勭悊妯″瀷锛屼笉鎻愪緵璺ㄧ嚎绋嬫棤閿侀槦鍒椼€佸懡浠ゆ挙閿€銆佷簨鍔″洖婊氭垨鑷姩骞惰璋冨害銆傚懡浠ゅけ璐ユ椂锛屽け璐ュ墠鐨勫懡浠ゅ凡缁忕敓鏁堬紱璋冪敤鏂瑰簲渚濇嵁閿欒涓婁笅鏂囧喅瀹氭槸鍚﹂噸寤?World 鎴栨彁浜よˉ鍋垮懡浠ゃ€?
## 鍚庣画琛旀帴

- M2-07锛氬湪 10000 瀹炰綋鍦烘櫙涓嬫祴閲忓垱寤恒€佹煡璇€佹洿鏂般€侀攢姣佸拰鍛戒护搴旂敤寮€閿€锛?- M2 閲岀▼纰戯細缁撳悎 Events銆丼cheduler 鍜屽帇鍔涙祴璇曞畬鎴?`v0.2-ecs` 楠屾敹銆