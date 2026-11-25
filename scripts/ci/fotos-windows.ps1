# Fotografa as páginas do instalador (só no ensaio).
param([string]$Instalador, [string]$Saida)
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
New-Item -ItemType Directory -Force $Saida | Out-Null

function Foto([string]$Nome) {
    Start-Sleep 2
    $b = [Windows.Forms.Screen]::PrimaryScreen.Bounds
    $im = New-Object Drawing.Bitmap $b.Width, $b.Height
    [Drawing.Graphics]::FromImage($im).CopyFromScreen($b.Location, [Drawing.Point]::Empty, $b.Size)
    $im.Save("$((Resolve-Path $Saida).Path)\$Nome.png")
}

$p = Start-Process $Instalador -ArgumentList '/CURRENTUSER', '/LANG=pt', "/DIR=$env:RUNNER_TEMP\fotos" -PassThru
Start-Sleep 5
Foto '1-boas-vindas'
[Windows.Forms.SendKeys]::SendWait('{ENTER}')
Foto '2-opcoes'
[Windows.Forms.SendKeys]::SendWait('{ENTER}')
Foto '3-pronto'
[Windows.Forms.SendKeys]::SendWait('{ENTER}')
Start-Sleep 8
Foto '4-fim'
Get-Process | Where-Object { $_.Path -like '*duck-sheets*' } | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep 2
if (Test-Path "$env:RUNNER_TEMP\fotos\unins000.exe") {
    Start-Process "$env:RUNNER_TEMP\fotos\unins000.exe" -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES' -Wait
}
