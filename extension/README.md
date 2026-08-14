# oxid — extensão de navegador

Um clique encurta a página aberta e põe o link curto na área de transferência.

```bash
npm install
npm run build     # gera dist/chrome, dist/firefox e dist/safari
npm run check     # só checagem de tipos
```

## Por que popup, depois de decidir que não teria popup

A primeira versão encurtava direto do clique no ícone e escrevia na área de
transferência **injetando um script na página onde você estava**. O popup foi
descartado de propósito: seria mais uma janela entre querer o link e ter o link.

Funcionou em mais ou menos metade da web. Injeção é recusada nas páginas do
próprio navegador, é recusada antes de `activeTab` ser concedida, e — a que
realmente pegou — a escrita da área de transferência dentro do script injetado
exige a página em foco e uma permissão (`clipboardWrite`) que estava
documentada aqui e **nunca tinha sido declarada** no manifest. Todas essas
falhas chegavam como o mesmo `false` silencioso, e o único retorno era um
caractere no badge.

Popup é página de extensão. Tem documento próprio, está em foco por definição, e
consegue dizer uma frase em vez de um caractere. O clique que ele custa paga uma
classe inteira de bug.

O botão direito num link continua pelo caminho antigo — ali não há janela para
abrir. É o caminho mais fraco, e sobrevive porque um clique com o botão direito
num link acontece sempre *dentro* de uma página, que é o caso que a injeção
resolve bem.

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

O token fica em `storage.local`, que é por navegador e por perfil. O popup nunca
o escreve de volta no campo: preenchê-lo colocaria uma credencial viva no DOM
toda vez que ele abre, sem servir para nada.

Para trocar ou remover o token: **Configurações**, no rodapé do popup. O botão
direito no ícone → Preferências abre a mesma tela, porque `options_ui` aponta
para o mesmo arquivo.

## Permissões, e por que são estas

- **`activeTab`** dá acesso à aba **apenas no clique**, e é tudo que isto
  precisa. `host_permissions: ["<all_urls>"]` — o caminho mais fácil — seria
  pedir para ler qualquer página que a pessoa visite, atrasar a revisão das
  lojas e transformar um comprometimento da extensão num vazamento do histórico
  inteiro.
- **`scripting`** existe só pelo menu de contexto: `navigator.clipboard` **não
  existe** num service worker MV3, porque não há documento que possa deter a
  seleção. O popup não precisa dela — tem documento próprio.
- **`clipboardWrite`** é o que permite `execCommand("copy")` fora de um
  manipulador de evento do usuário. `navigator.clipboard.writeText` rejeita sem
  ativação transitória, e a ativação de abrir o popup já foi gasta quando o
  servidor responde.
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

No primeiro clique no ícone o popup abre já no formulário. Cole ali um token
criado em **Conta → Tokens de API** no site.

## Estado

Funciona ponta a ponta e ainda não foi publicada em loja nenhuma. O CI de
publicação automática é o próximo passo, e os dois primeiros itens dele são as
credenciais de loja — que ainda não existem.
