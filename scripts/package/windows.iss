; Instalador do Windows (Inno Setup 6.6 ou mais novo). Chamado por scripts/package.sh:
;   ISCC /DVersion=1.0.0-beta.1 /DNumber=1.0.0 /DStage=... /DArt=... /DOut=... windows.iss
#define App "Duck Sheets"
#define Id "duck-sheets"
#define Exe "duck-sheets.exe"
#define Prog "duck-sheets.Arquivo"
#define Url "https://github.com/SrWalkerB/duck-office"

[Setup]
AppId={{8C0C1B52-6D0E-4B8F-9B8B-2B8F7A0E5D31}
AppName={#App}
AppVersion={#Version}
AppVerName={#App} {#Version}
AppPublisher=duck-office
AppPublisherURL={#Url}
AppSupportURL={#Url}/issues
AppUpdatesURL={#Url}/releases
VersionInfoVersion={#Number}
VersionInfoProductTextVersion={#Version}
VersionInfoDescription={#App} - instalador
VersionInfoCopyright=Licença MIT
; Só para o usuário, sem pedir senha de administrador; dá para escolher "todos".
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
DefaultDirName={autopf}\{#App}
DisableDirPage=auto
DisableProgramGroupPage=yes
DisableWelcomePage=no
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
ChangesAssociations=yes
CloseApplications=yes
RestartApplications=no
SetupIconFile={#Stage}\icon.ico
UninstallDisplayIcon={app}\{#Exe}
UninstallDisplayName={#App}
WizardStyle=modern dynamic
WizardImageFile={#Art}\lateral-164.png,{#Art}\lateral-246.png,{#Art}\lateral-328.png,{#Art}\lateral-410.png
WizardSmallImageFile={#Art}\topo-55.png,{#Art}\topo-83.png,{#Art}\topo-110.png,{#Art}\topo-138.png
Compression=lzma2/max
SolidCompression=yes
OutputDir={#Out}
OutputBaseFilename={#Id}-{#Version}-windows-x64-instalador

[Languages]
Name: "pt"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
pt.Assoc=Aparecer no "Abrir com" de .xlsx, .xlsm, .xlsb e .csv
en.Assoc=Show up in "Open with" for .xlsx, .xlsm, .xlsb and .csv
pt.AssocGroup=Integração com o Windows:
en.AssocGroup=Windows integration:
pt.FileType=Planilha ({#App})
en.FileType=Spreadsheet ({#App})
pt.AppAbout=Planilhas gigantes, sem travar.
en.AppAbout=Huge spreadsheets, no freezing.

[Messages]
pt.WelcomeLabel2=Este assistente vai instalar o [name/ver].%n%nAbre, calcula e salva planilhas do Excel gigantes sem travar, sem conta e sem anúncio.%n%nEsta é uma versão beta: guarde uma cópia das planilhas importantes.
en.WelcomeLabel2=This will install [name/ver].%n%nOpens, calculates and saves huge Excel workbooks without freezing. No account, no ads.%n%nThis is a beta: keep a copy of your important workbooks.

[Tasks]
Name: "assoc"; Description: "{cm:Assoc}"; GroupDescription: "{cm:AssocGroup}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Stage}\{#Exe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "{#Stage}\LICENCAS-DE-TERCEIROS.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#App}"; Filename: "{app}\{#Exe}"; Comment: "{cm:AppAbout}"
Name: "{autodesktop}\{#App}"; Filename: "{app}\{#Exe}"; Tasks: desktopicon

[Registry]
; Pelo nome no Executar (Win+R).
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\{#Exe}"; ValueType: string; ValueData: "{app}\{#Exe}"; Flags: uninsdeletekey
; O mesmo que "Adicionar ao Abrir com" do menu Arquivo. O programa padrão não muda.
Root: HKA; Subkey: "Software\Classes\{#Prog}"; ValueType: string; ValueData: "{cm:FileType}"; Flags: uninsdeletekey; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\{#Prog}\DefaultIcon"; ValueType: string; ValueData: """{app}\{#Exe}"",0"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\{#Prog}\shell\open\command"; ValueType: string; ValueData: """{app}\{#Exe}"" ""%1"""; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#Exe}"; Flags: uninsdeletekey; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Applications\{#Exe}\shell\open\command"; ValueType: string; ValueData: """{app}\{#Exe}"" ""%1"""; Tasks: assoc
#define Ext(str E) \
  "Root: HKA; Subkey: ""Software\Classes\." + E + "\OpenWithProgids""; ValueType: string; ValueName: """ + Prog + """; ValueData: """"; Flags: uninsdeletevalue; Tasks: assoc" + NewLine + \
  "Root: HKA; Subkey: ""Software\Classes\Applications\" + Exe + "\SupportedTypes""; ValueType: string; ValueName: ""." + E + """; ValueData: """"; Tasks: assoc" + NewLine + \
  "Root: HKA; Subkey: ""Software\" + Id + "\Capabilities\FileAssociations""; ValueType: string; ValueName: ""." + E + """; ValueData: """ + Prog + """; Tasks: assoc"
#emit Ext("xlsx")
#emit Ext("xlsm")
#emit Ext("xlsb")
#emit Ext("csv")
; Em Configurações › Aplicativos padrão.
Root: HKA; Subkey: "Software\{#Id}"; Flags: uninsdeletekey; Tasks: assoc
Root: HKA; Subkey: "Software\{#Id}\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "{#App}"; Tasks: assoc
Root: HKA; Subkey: "Software\{#Id}\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "{cm:AppAbout}"; Tasks: assoc
Root: HKA; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "{#App}"; ValueData: "Software\{#Id}\Capabilities"; Flags: uninsdeletevalue; Tasks: assoc

[Run]
Filename: "{app}\{#Exe}"; Description: "{cm:LaunchProgram,{#App}}"; Flags: nowait postinstall skipifsilent
; "Reiniciar e atualizar": o programa chama o instalador com /reabrir=1.
Filename: "{app}\{#Exe}"; Flags: nowait; Check: Reopen

[Code]
function Reopen: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:reabrir|0}') = '1');
end;
