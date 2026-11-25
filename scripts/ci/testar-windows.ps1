# Instala o pacote numa máquina limpa, abre, atualiza (no ensaio) e desinstala.
param([string]$Versao, [string]$Pacotes, [string]$Ensaio)
$ErrorActionPreference = 'Stop'

function Confere([bool]$Ok, [string]$O) {
    if (-not $Ok) { throw "falhou: $O" }
    Write-Host "ok  $O"
}

# Programa de janela: só termina de escrever quando alguém espera por ele.
function Roda([string]$Exe, [string[]]$Opcoes) {
    $saida = [IO.Path]::GetTempFileName()
    $erro = [IO.Path]::GetTempFileName()
    $p = Start-Process $Exe -ArgumentList $Opcoes -Wait -PassThru -NoNewWindow -RedirectStandardOutput $saida -RedirectStandardError $erro
    $texto = ((Get-Content $saida, $erro -Encoding UTF8) -join "`n").Trim()
    Write-Host "    $texto"
    return @{ Codigo = $p.ExitCode; Texto = $texto }
}

$instalador = (Resolve-Path "$Pacotes\duck-sheets-$Versao-windows-x64-instalador.exe").Path
$pasta = "$env:LOCALAPPDATA\Programs\Duck Sheets"
$exe = "$pasta\duck-sheets.exe"
$log = "$env:RUNNER_TEMP\instalar.log"

Write-Host '--- portátil'
Expand-Archive "$Pacotes\duck-sheets-$Versao-windows-x64-portatil.zip" portatil
$r = Roda (Resolve-Path portatil\duck-sheets.exe).Path @('--version')
Confere ($r.Texto -eq "Duck Sheets $Versao") 'a versão portátil abre'
Confere (Test-Path portatil\LICENCAS-DE-TERCEIROS.txt) 'licenças no portátil'

Write-Host '--- instalar'
$p = Start-Process $instalador -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER', "/LOG=$log" -Wait -PassThru
if ($p.ExitCode -ne 0) { Get-Content $log -Tail 40 }
Confere ($p.ExitCode -eq 0) 'instalação silenciosa, sem administrador'
Confere (Test-Path $exe) "programa em $pasta"
$r = Roda $exe @('--version')
Confere ($r.Texto -eq "Duck Sheets $Versao") 'o programa instalado abre'
Confere (Test-Path "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Duck Sheets.lnk") 'atalho no menu Iniciar'
$classes = 'HKCU:\Software\Classes'
foreach ($e in 'xlsx', 'xlsm', 'xlsb', 'csv') {
    $v = (Get-Item "$classes\.$e\OpenWithProgids").GetValueNames()
    Confere ($v -contains 'duck-sheets.Arquivo') "Abrir com de .$e"
}
Confere ((Get-Item "$classes\duck-sheets.Arquivo\shell\open\command").GetValue('') -eq "`"$exe`" `"%1`"") 'comando de abrir'
Confere ((Get-Item 'HKCU:\Software\RegisteredApplications').GetValue('Duck Sheets') -eq 'Software\duck-sheets\Capabilities') 'aplicativos padrão'
$info = (Get-Item $exe).VersionInfo
Confere ($info.ProductName -eq 'Duck Sheets' -and $info.ProductVersion -eq $Versao) 'informações de versão'

if ($Ensaio -and (Test-Path "$Ensaio\chave.key")) {
    Write-Host '--- atualizar'
    cargo install rsign2 --locked --version $env:RSIGN --quiet
    $origem = (New-Item -ItemType Directory -Force atualizacao).FullName
    Copy-Item $instalador $origem
    $f = Get-Item $instalador
    $sha = (Get-FileHash $instalador -Algorithm SHA256).Hash
    $texto = "duck-sheets 1`nversao=99.0.0-beta.1`narquivo=windows-instalador|$($f.Name)|$($f.Length)|$sha`n"
    [IO.File]::WriteAllText("$origem\beta.txt", $texto)
    Copy-Item "$origem\beta.txt" "$origem\estavel.txt"
    foreach ($c in 'beta', 'estavel') { rsign sign -W -s "$Ensaio\chave.key" -x "$origem\$c.txt.minisig" "$origem\$c.txt" }
    $env:DUCK_ATUALIZACAO = 'file:///' + $origem.Replace('\', '/')
    $env:DUCK_RECUPERACAO = "$env:RUNNER_TEMP\estado"

    # Um byte trocado no manifesto: nada acontece.
    [IO.File]::WriteAllText("$origem\beta.txt", $texto.Replace('99.0.0', '99.0.1'))
    Copy-Item "$origem\beta.txt" "$origem\estavel.txt" -Force
    $r = Roda $exe @('--atualizar')
    Confere ($r.Codigo -ne 0 -and $r.Texto -match 'assinatura') 'manifesto adulterado é recusado'
    [IO.File]::WriteAllText("$origem\beta.txt", $texto)
    Copy-Item "$origem\beta.txt" "$origem\estavel.txt" -Force

    # Instalador adulterado: idem.
    $bytes = [IO.File]::ReadAllBytes("$origem\$($f.Name)")
    $bytes[1000] = $bytes[1000] -bxor 1
    [IO.File]::WriteAllBytes("$origem\$($f.Name)", $bytes)
    $r = Roda $exe @('--atualizar')
    Confere ($r.Codigo -ne 0 -and $r.Texto -match 'não confere') 'instalador adulterado é recusado'
    Copy-Item $instalador $origem -Force

    Remove-Item "$pasta\LICENSE.txt"
    $r = Roda $exe @('--atualizar')
    Confere ($r.Codigo -eq 0 -and $r.Texto -match '99\.0\.0-beta\.1') 'baixou e começou a instalar'
    for ($i = 0; $i -lt 60 -and -not (Test-Path "$pasta\LICENSE.txt"); $i++) { Start-Sleep 1 }
    $atual = "$env:RUNNER_TEMP\estado\atualizacao\instalador.log"
    if (-not (Test-Path "$pasta\LICENSE.txt") -and (Test-Path $atual)) { Get-Content $atual -Tail 40 }
    Confere (Test-Path "$pasta\LICENSE.txt") 'a atualização repôs os arquivos'
    for ($i = 0; $i -lt 30 -and (Get-Process duck-sheets-* -ErrorAction SilentlyContinue); $i++) { Start-Sleep 1 }
    $r = Roda $exe @('--version')
    Confere ($r.Texto -eq "Duck Sheets $Versao") 'abre depois de atualizar'
}

Write-Host '--- desinstalar'
Start-Process "$pasta\unins000.exe" -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait
for ($i = 0; $i -lt 20 -and (Test-Path $exe); $i++) { Start-Sleep 1 }
Confere (-not (Test-Path $exe)) 'programa removido'
Confere (-not (Test-Path "$classes\duck-sheets.Arquivo")) 'registro limpo'
$v = (Get-Item 'HKCU:\Software\RegisteredApplications' -ErrorAction SilentlyContinue)
Confere (-not $v -or $v.GetValueNames() -notcontains 'Duck Sheets') 'aplicativos padrão limpos'
