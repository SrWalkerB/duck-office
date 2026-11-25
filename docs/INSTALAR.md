# Instalar o Duck Sheets

Os instaladores ainda não têm a assinatura paga da Microsoft e da Apple: na primeira vez, o sistema avisa que não conhece o programa.

### Windows 10 e 11

1. Abra o `...-instalador.exe`. Se aparecer "O Windows protegeu o computador", clique em **Mais informações › Executar assim mesmo**.
2. A instalação é só para o seu usuário e não pede senha de administrador.

Portátil: extraia o `.zip` e abra o `duck-sheets.exe`.

### macOS 11 ou mais novo

1. Abra o `.dmg` e arraste o Duck Sheets para **Aplicativos**.
2. Na primeira vez, o macOS recusa: vá a **Ajustes do Sistema › Privacidade e Segurança** e clique em **Abrir Mesmo Assim**.

### Linux (x86_64, glibc 2.35 ou mais nova)

- **AppImage**: `chmod +x duck-sheets-*.AppImage` e abra.
- **Debian e Ubuntu**: `sudo apt install ./duck-sheets-*.deb`
- **Fedora**: `sudo dnf install ./duck-sheets-*.rpm`

Precisa de uma placa com Vulkan (ou do `mesa-vulkan-drivers`).

### Atualizações

O instalador do Windows (só para o seu usuário), o `.app` do macOS e o AppImage se atualizam sozinhos: a versão nova é baixada em segundo plano, conferida e instalada quando a última janela fecha. Dá para desligar em **Configurações** e para procurar na hora em **Arquivo › Procurar atualizações**. Os outros só avisam que há versão nova.

O programa só pergunta ao GitHub qual é a versão mais nova; nada sobre você ou as suas planilhas é enviado.
