-- Duas colunas em `users`, para as etapas 14 e 15.
--
-- Numa migration só porque são dois `ALTER TABLE` na mesma tabela pequena, e
-- separá-las custaria duas travas de esquema para ganhar nada.

-- NULL = não confirmado. Um timestamp em vez de um booleano: "quando" responde
-- a mais perguntas que "se", e nenhuma delas precisa de outra coluna depois —
-- quanto tempo alguém leva para confirmar, quantos nunca confirmaram, e a partir
-- de quando a conta passou a poder entrar.
ALTER TABLE users ADD COLUMN email_verified_at TIMESTAMPTZ NULL;

-- Contas que já existem entram confirmadas.
--
-- É decisão de produto, não conveniência de migration: a confirmação passa a
-- barrar o login, e marcar as contas atuais como não confirmadas trancaria
-- do lado de fora gente que se cadastrou quando isso não era exigido. Elas
-- provaram o endereço da forma que existia na época — nenhuma.
--
-- `now()` e não a data de criação: o que a coluna afirma é o momento em que a
-- conta passou a ser considerada confirmada, e para estas foi agora.
UPDATE users SET email_verified_at = now() WHERE email_verified_at IS NULL;

-- Sem isso, "autenticado" viraria "qualquer conta" no painel da etapa 14.
--
-- Uma coluna booleana e não uma tabela de papéis: existem dois papéis e não há
-- um terceiro à vista. Uma tabela de papéis com dois valores é a generalização
-- que se paga hoje e só talvez se use — e trocar depois é um `ALTER` numa tabela
-- de contas, não de cliques.
ALTER TABLE users ADD COLUMN is_admin BOOLEAN NOT NULL DEFAULT FALSE;

-- O painel filtra por `is_admin` e nada mais; sem índice parcial isso é um scan
-- na tabela de contas a cada carregamento. Parcial porque só as verdadeiras
-- interessam, e elas são uma fração desprezível das linhas.
CREATE INDEX users_is_admin_idx ON users (id) WHERE is_admin;
