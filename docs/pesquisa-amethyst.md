# Pesquisa: Amethyst Mod Manager

Fonte: https://github.com/ChrisDKN/Amethyst-Mod-Manager (Python + Qt, GPL-3.0 — mesma
licença daqui; dá para portar trechos com crédito). Lido em 2026-10-11, v2.5.4.

## O que aproveitar

- **Link `nxm://`** — `nxm://<domínio>/mods/<mod>/files/<arquivo>?key=..&expires=..`. Conta
  gratuita só baixa por esse link (chave de uso único); premium gera link direto pela API.
  Coleções: `nxm://<domínio>/collections/<slug>[/revisions/<id>]`.
- **Instância única** — o navegador abre um processo novo com o link; ele repassa para o app
  aberto por socket Unix. O problema real deles: o processo do navegador às vezes resolve outro
  caminho de socket (Flatpak, `XDG_RUNTIME_DIR` perdido), então tentam vários caminhos e caem em
  `/tmp/<app>-<uid>.sock`. Esse processo não tem janela: registram tudo em `logs/nxm.log`.
- **Chave da API** — no chaveiro do sistema (Secret Service), com arquivo como reserva quando o
  DBus/keyring falha (comum no SteamOS). Também têm login OAuth 2.0 + PKCE, mas exige um
  `client_id` registrado no Nexus ("amethyst"); para isso teríamos que pedir o nosso.
- **Detecção de jogos** — Steam, Heroic, Lutris e Faugus.
- **Regras por jogo** — `src/Games/<jogo>/` (36 definições, "quase 100 jogos" contando os
  genéricos BepInEx/UE5). Bom mapa do que cada jogo precisa.
- **Stardew** — arquivos de raiz (SMAPI) não podem ser symlink: o apphost do .NET procura o
  runtime ao lado do executável.

## Onde divergimos de propósito

- **Implantação**: eles usam pasta de staging + hardlink/symlink/VFS e perfis, estilo MO2, com
  "restaurar" que devolve a pasta do jogo ao original. Mais seguro, bem mais complexo. Aqui por
  enquanto é cópia direta e desligar = mover para `.linex-disabled/`.
- **Interface**: a deles é densa, estilo MO2. A nossa é para quem nunca modou.
