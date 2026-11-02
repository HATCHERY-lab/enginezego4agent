# Бриф: агент engine4agent (op-box)

Ты ведёшь engine4agent (e4a): наш Rust-порт durable Pi. Руководитель — hatcheryweb,
отчёты шлёшь ему в fed-комнату. План:
https://github.com/HATCHERY-lab/hatchery-websession-docs/blob/main/docs/plans/engine4agent.md
Прочти §1–§10 целиком до начала работы.

Суть: берём @earendil-works/pi-durable 1.5.0 (MIT, тег v1.5.0 = f9a26365) за
зафиксированную базу, переписываем на Rust и дальше развиваем сами, на upstream не
оглядываемся. Низкоуровневое своё не пишем: genai (LLM), rusqlite, tokio,
json-patch, similar, notify, agent-client-protocol, код pi-env daemon.

## Задача сейчас: Э0 — эталон с оригинала (1–1.5 дня)

1. Node ≥ 22.19. В фед-дереве engine4agent заведи папку e4a-ref/ и установи пин-версии
   с --ignore-scripts: pi-durable, pi-ai, chord, все 1.5.0. package-lock закоммить.
2. Ключ бери только из env-переменной, которую назовёт владелец (имя пришлю).
   Не пиши его в файлы, argv, логи и отчёты. Не печатай env.
3. Напиши host.mjs (~100 строк): Harness.open на SQLite-хранилище → root() →
   submit → wait. В JSONL пиши observeCommits, значения Conversation.watch() и
   pi.usage.
4. Сценарии, каждый в свой fixture (fixtures/<имя>.jsonl):
   a) вопрос-ответ; b) ответ с tool read; c) kill посреди стрима → перезапуск → ход
   доведён; d) kill во время tool; e) fork на записи и продолжение; f) abort
   посреди ответа; g) смена модели/effort между ходами; h) powershell tool.
   Для a/b отдельно сохрани сырой SSE провайдера.
5. Перед коммитом вычисти ключ и заголовки авторизации. Проверь grep'ом по имени
   переменной и по префиксу ключа. Ни одного секрета в git.
6. Коммить в фед-дерево engine4agent. Автор knyaz-tishini, без Co-Authored-By,
   без force и без переписывания истории.
7. Отчёт мне: версии, команды, какие сценарии прошли и какие нет, пути fixtures,
   sha коммита. Коротко, по-русски.

## Дальше (только после моего «ок» по Э0)

Э1 e4a-types + e4a-store (SQLite/memory + conformance-тесты Storage) → Э2 session →
Э3 harness на genai → Э4 tools/env → Э5 бинарь `e4a acp` и провайдер в g4a →
Э6 брокер ключа → Э7 advisor. Каждый этап: ветка в фед-дереве, тесты, отчёт, моё
ревью. После каждого этапа — контрольная точка в HATCHERY-lab/engine4agent (обычный
push, без force).

## Правила

- Шапка на каждом модуле, портированном близко к оригиналу:
  `// Portions derived from pi-durable 1.5.0 (packages/durable/src/<file>), MIT,
  © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.`
- Сборки только на op-box. После сборок cargo clean. На чужие машины ничего не ставь.
- Не трогать: сессию 17194c50, m4a-agent.exe, ноду op-box и её конфиги, секреты.
  Ничего не удалять без моего «да».
- Без релизов на crates.io и npm, без бампов чужих версий.
- Застрял больше чем на 2 часа — пиши мне, что пробовал.
