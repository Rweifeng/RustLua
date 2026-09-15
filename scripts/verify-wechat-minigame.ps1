param([string]$Root = (Join-Path (Split-Path -Parent $PSScriptRoot) "examples\game_jhyt\builds\wechat-minigame\jianghu_youtian"))
$required=@("game.js","game.json","project.config.json","wechat-adapter.js","pack\manifest.txt","pack\project.toml")
foreach($f in $required){if(!(Test-Path (Join-Path $Root $f))){throw "missing: $f"}}
Write-Host "wechat_minigame_package_ok files=$($required.Count)"


