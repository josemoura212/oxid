# oxid — extensão de navegador

Um clique encurta a página aberta e põe o link curto na área de transferência.
Sem popup: o popup seria mais uma janela entre querer o link e ter o link, e os
quatro passos que isto substitui — sair da página, abrir o oxid, colar, copiar —
são a razão de a extensão existir.

```bash
npm install
npm run build     # gera dist/chrome, dist/firefox e dist/safari
npm run check     # só checagem de tipos
```

## Um código, três manifests

O código é idêntico nos três navegadores. **Toda** a diferença mora no manifest,
e mantê-la ali em vez de em `if (isFirefox)` é o que impede "funciona no Chrome"
de virar uma categoria de bug.

| | Chrome / Edge | Firefox | Safari |
|---|---|---|---|
| Background | `service_worker` | `scripts` | `service_worker` |
| Identidade | atribuída pela loja | `browser_specific_settings.gecko.id` | pelo bundle Xcode |

## Por que token e não o cookie de sessão

Extensão não compartilha cookie com o site de forma confiável entre navegadores.
E o token é melhor por mérito próprio: é revogável sozinho, então desinstalar
daqui — ou perder o notebook onde ela está — não desloga ninguém do site.

O token fica em `storage.local`, que é por navegador e por perfil. A página de
opções nunca o escreve de volta no campo: preenchê-lo colocaria uma credencial
viva no DOM toda vez que a página abre, sem servir para nada.

## Permissões, e por que são estas

- **`activeTab`** dá acesso à aba **apenas no clique**, e é tudo que isto
  precisa. `host_permissions: ["<all_urls>"]` — o caminho mais fácil — seria
  pedir para ler qualquer página que a pessoa visite, atrasar a revisão das
  lojas e transformar um comprometimento da extensão num vazamento do histórico
  inteiro.
- **`scripting`** existe por uma razão específica: `navigator.clipboard` **não
  existe** num service worker MV3, porque não há documento que possa deter a
  seleção. Escrever na área de transferência exige injetar na aba ativa.
- **`host_permissions`** restrito a `https://oxid.uk/*`. Isso também é o que
  dispensa CORS: o fetch parte do service worker com permissão para aquele host,
  então não há preflight a configurar no servidor.

  Vale registrar por que **não** foi CORS: no Firefox a origem de uma extensão é
  um UUID **aleatório por instalação**. Não existe lista para autorizar — só
  daria para liberar `moz-extension://*`, que é liberar qualquer extensão de
  qualquer usuário.

## Safari precisa de um passo a mais

Safari carrega uma extensão web, mas só empacotada num app. A conversão é da
Apple e exige Xcode em macOS:

```bash
xcrun safari-web-extension-converter dist/safari
```

Isso é o que impede o Safari de entrar no mesmo CI dos outros dois: o passo
depende de um runner macOS com Xcode, e assinatura de app. Chrome e Firefox
publicam por API a partir de qualquer runner.

## Carregar sem publicar

- **Chrome/Edge** — `chrome://extensions`, ative "modo desenvolvedor", "carregar
  sem compactação", aponte para `dist/chrome`.
- **Firefox** — `about:debugging#/runtime/this-firefox`, "carregar extensão
  temporária", escolha `dist/firefox/manifest.json`. Some ao fechar o navegador.

Depois abra as opções da extensão e cole um token criado em **Conta → Tokens de
API** no site.

## Estado

Funciona ponta a ponta e ainda não foi publicada em loja nenhuma. O CI de
publicação automática é o próximo passo, e os dois primeiros itens dele são as
credenciais de loja — que ainda não existem.
