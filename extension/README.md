# oxid — extensão de navegador

Um clique encurta a página aberta e põe o link curto na área de transferência.

```bash
npm install
npm run build     # gera dist/chrome, dist/firefox e dist/safari
npm run package   # o mesmo, mais os .zip prontos para as lojas
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

## Enviar para a addons.mozilla.org

```bash
npm run package
npx addons-linter dist/oxid-firefox-0.1.0.zip
```

O `.zip` vai em `addons.mozilla.org/developers/` → "Enviar nova extensão".

**O manifest tem que estar na raiz do arquivo.** `zip -r saida.zip pasta` guarda
`pasta/manifest.json`, e a AMO recusa com um erro sobre manifest ausente que não
diz nada sobre a causa real. Por isso o `package.mjs` compacta de dentro do
diretório.

### `data_collection_permissions` é obrigatório para extensão nova

A Mozilla passou a exigir a declaração do que a extensão coleta. A nossa
transmite **a URL da aba** para `oxid.uk` — é o produto —, o que na taxonomia
deles é `browsingActivity`. Nada mais sai daqui: o token fica em `storage.local`
e só viaja de volta para o servidor que o emitiu.

O `addons-linter` avisa duas vezes que `strict_min_version: 115.0` é anterior ao
suporte da chave — **Firefox 140** no desktop, **142** no Android. É aviso, não
erro, e a escolha é deliberada: subir o mínimo excluiria quem está no ESR 115 em
troca de uma tela de consentimento que essas versões não renderizam. A declaração
continua aparecendo na página da loja para todos.

### Safari não entra aqui

Ele precisa de app bundle, não de zip — ver a seção acima.

## Estado

Funciona ponta a ponta. O pacote do Firefox passa no `addons-linter` com zero
erros e está pronto para envio manual.

O CI de publicação automática é o próximo passo. A AMO tem API
(`/api/v5/addons/`) com chave JWT gerada no Developer Hub, e o ID fixo que ela
exige já está no manifest (`oxid@oxid.uk`) — o que falta é a credencial.
