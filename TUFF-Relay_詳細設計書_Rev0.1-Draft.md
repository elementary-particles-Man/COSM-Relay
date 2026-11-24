# TUFF-Relay 詳細設計書

| 項目 | 内容 |
|---|---|
| 文書番号 | TR-DD-001 |
| 版 | Rev.0.1-Draft |
| 作成日 | 2026-11-24 JST |
| 親文書 | TUFF-Relay 基本設計書 Rev.0.1-Draft（TR-BD-001） |
| 対象 | ローカル開発支援RelayのRust Core・状態・接続・運用・試験 |

本書の具体的な型、コマンド、永続形式は実装提案 [P]。外部接続と既存メッセージ形式は基本設計のR-01〜R-06に従い未決 [R] とする。F（既決）を実装都合で変更しない。

## 1. コンポーネント構成 [P]

初版は単一Rust workspace・単一実行バイナリを原則とする。初期実装でネットワークサービス、DBサーバー、別言語ランタイムの導入を必須にしない。

```text
TUFF-Relay/
  Cargo.toml
  Cargo.lock
  src/
    main.rs                 CLIの入口
    config.rs               登録済みレーンと権限の読取・検証
    core/
      mod.rs
      envelope.rs           ID・メッセージ型・原文ハッシュ
      reducer.rs            純粋な状態遷移
      lane.rs               レーン別処理／ロック
      reconcile.rs          不確定状態の照合結果の適用
    journal.rs              永続イベントと原文の保存
    runtime.rs              アダプタ駆動・I/O・キャンセル
    adapters/
      mod.rs
      mock.rs               P1テスト用の監督・ワーカー
      codex.rs              公式app-server接続（P2）
      supervisor_mcp.rs     MCP接続候補（R-01成立後）
    operator.rs             ローカル運転操作
  tests/
    core_transitions.rs
    replay.rs
    fault_injection.rs
    lanes.rs
    mock_loop.rs
    adapter_contract.rs
  docs/
    BASIC.md
    DETAIL.md
    DECISIONS.md
```

Rustの標準ライブラリと、必要性が確認された最小限のRust依存を使用する。JSONにはserde/serde_json、暗号学的ハッシュにはSHA-256のRust実装を候補とする。I/Oランタイム・MCPライブラリ・ロック用crateの選定は接続PoCの要件に従う。公式Codexや公式トンネルクライアントは外部実行物として扱い、Relay製品内部の実装言語の例外とはしない。`unsafe`を必要とする独自実装は初版対象外（依存crate内部は別途確認）。

## 2. レーン登録・セッション同一性

### 2.1 設定項目

設定はローカル運転員が作成する。以下はTOML形式の例であり、実際のユーザー設定ではない。認証トークンを直書きしない。

```toml
schema_version = 1
state_dir = "/home/USER/.local/state/tuff-relay"

[[lane]]
id = "tuff-tag"
repo_root = "/home/USER/src/TUFF-Tag"
supervisor_binding = "registered-tuff-tag-supervisor"
worker_thread_id = "existing-codex-thread-id"
worker_adapter = "codex_app_server"
supervisor_adapter = "mcp" # R-01成立後のみ有効
```

| 項目 | 制約 |
|---|---|
| lane ID | ASCII英小文字・数字・ハイフン。1〜64文字。登録時に一意 |
| repo_root | 存在する絶対パス。起動時に正規化し、既存Git作業ツリーを照合 |
| supervisor_binding | 運転員が対象会話と対応付けた接続識別子。LLMが自由入力するルーティング値を信頼しない |
| worker_thread_id | 正式なCodex接続で確認済みのスレッド識別子。空欄・不一致なら配送不可 |
| binding epoch | 明示的な再登録時のみ増加。旧接続からの書込みを拒否 |

同じリポジトリの別名パスやシンボリックリンク経由の二重登録を拒否する。レーンごとに排他的な運転ロックを取得し、同じワーカーまたは作業場所を同時に制御しない。外部のCodex TUIや別プログラムの同時書込みを自動で完全検知できると仮定せず、初回切替時と復旧時に運転員がアイドル・単独所有を確認する。

監督側接続に信頼できる会話IDが提供されない場合は、レーンごとに分離した認証／接続スコープと明示的バインドを用いる（R-01）。接続の同一性をMCPが保証しない場合、会話固定は運用上の制約に留まり、厳密な自動誤配送防止を満たしたと判定しない。

## 3. 通信契約

### 3.1 原文と封筒を分離する [P/R-04]

既存の監督GPTのTask JSONをRelay用の新しいJSON形式へ置換しない。既存JSONのUTF-8原文をペイロードとして保存し、識別子・配送状態を別に付す。既存フォーマットの変更が必要な場合はR-04で決定する。

| フィールド | 型（案） | 意味 |
|---|---|---|
| protocol_version | u16 | 初版1。未知版を推測処理しない |
| lane_id | String | 登録済みレーン。接続から確定 |
| binding_epoch | u64 | 接続世代 |
| run_id | String | 人間が開始した運転単位。Relay発行 |
| message_id | String | 発信側または受付時に確定した一意ID。再送時に維持 |
| task_id | String | タスク固有ID。再送・報告で維持 |
| in_reply_to | Option<String> | 直前報告／判断要求への対応付け |
| kind | enum | `task`, `report`, `decision_required`, `stop` |
| payload_utf8 | String | 既存のタスクJSON、報告本文などの原文 |
| payload_sha256 | [u8;32] | Relayが原文UTF-8バイトから算出 |

JSON-RPCまたはMCP上のエスケープは伝送符号化であり、復号後の本文は変えない。ハッシュの計算対象は原文バイト。正規化・整形・要約・改行追加をしない。JSON文字列中の改行、Unicode、空白を保持する。JSONの重複キーを含む曖昧なタスク入力の扱いはR-04で既存仕様と照合し、初期提案では拒否する。

`task_id`はタスクの受付前に発信元と合意できる冪等キーにする。既存JSONにIDがあるなら既存IDを優先して活用する。ない場合は監督に明示的な受付キーを持たせる案を採る（R-04）。Relayが受付後だけIDを生成すると受付応答喪失時に再送を識別できないため、初回送信から再送で同一キーを提示できることを必須とする。

### 3.2 例（既存Task JSONの代替ではない）

```json
{
  "protocol_version": 1,
  "lane_id": "tuff-tag",
  "binding_epoch": 1,
  "run_id": "run-example",
  "message_id": "msg-01",
  "task_id": "task-example-01",
  "in_reply_to": null,
  "kind": "task",
  "payload_utf8": "{\"task\":\"既決仕様の実装とテスト\"}"
}
```

`payload_sha256`は受信時に計算し、保存する。送信側のハッシュが提供されるなら照合する。例の`task`フィールドを実際の既存仕様と解釈しない。

### 3.3 完了報告・判断要求

Codexの確定した最終報告をそのまま取得して返送する。ストリーミング断片、途中のコードブロック、実行ログを完了報告とみなさない。

外部プロトコルのターン完了と作業内容の完了を区別する。`turn/completed`で成功状態でも、報告が仕様判断待ちなら作業完了ではない。判断の意味を自然言語からRelayが推定しない。既存の明示的な状態マーカー／JSON欄があれば使用する。なければ最小限の状態欄（例：`REPORT` / `DECISION_REQUIRED`）と原文本文を追加するか、監督が報告内容を評価し次に許可された操作を返す方式をR-04で決める。

判断要求には既存どおりサマリと決定すべき事項を含める。Relayは本文を生成しない。判断要求が監督に届いた後、R-03で承認済みの範囲に従って再開する。

## 4. 状態機械 [P]

### 4.1 Rust型の概念設計

```rust
// 擬似型定義。最終API・crate選定は詳細レビューで確定。
enum Phase {
    Idle,             // 未開始または前運転終了
    WaitTask,         // 監督の次タスク待ち
    Accepted,         // タスクを永続受付済み、外部未送信
    Dispatching,      // 送信意図を永続化済み、結果は未確定
    Running,          // ワーカーturn_id確認済み
    ReportReady,      // 報告を永続化済み、監督受領待ち
    WaitDecision,     // 判断要求を監督へ配送、次の判断待ち
    Reconcile,        // 外部の実行・配送状態が不明
    Stopped,          // 運転終了または手動停止、実行中ターンなし
}
struct LaneState {
    phase: Phase,
    enabled: bool,                 // 運転許可
    pause_requested: bool,         // 新規配送を抑止。既存処理状態を破壊しない
    run_id: Option<RunId>,
    active_task: Option<TaskId>,
    worker_turn: Option<TurnId>,
    pending_report: Option<MessageId>,
    last_acked_report: Option<MessageId>,
    journal_seq: u64,
    pending_approval: Option<ApprovalRef>,
}
```

`pause_requested`は実行状態とは別の運転フラグ。Running中の停止指示で実行状態をStoppedに上書きしない。`WaitDecision`は判断が必要なことを示し、Relayが何を選ぶかを示さない。

### 4.2 遷移表

| 現状態 | 入力／条件 | 永続化する事実 | 次状態 |
|---|---|---|---|
| Idle/Stopped | 運転員start、照合完了・レーン確認 | RunStarted | WaitTask |
| WaitTask | 正当な次タスク、前報告のACK整合 | TaskAccepted（原文含む） | Accepted |
| Accepted | 運転許可、未配送、接続正常 | DispatchIntent | Dispatching |
| Dispatching | 対応するワーカーturn ID受領 | WorkerBound | Running |
| Dispatching | 通信切断等で実行有無不明 | Uncertain | Reconcile |
| Running | 正常ターン完了、報告取得 | ReportStored（原文含む） | ReportReady |
| Running | 判断報告を取得しターン完了 | DecisionReportStored | ReportReady |
| Running | 承認要求 | ApprovalPending | Running（保留フラグ） |
| Running | ターン失敗・中断、または接続断 | WorkerOutcome / Uncertain | ReportReady（確定報告）またはReconcile（未確定） |
| ReportReady | 監督の明示受領ACK | ReportAcked | WaitTask またはWaitDecision |
| WaitDecision | 承認済みの判断応答、対応ID一致 | DecisionAccepted | Accepted（新しいtask IDで継続） |
| WaitTask | 監督の正常終了通知、または運転員停止 | RunStopped | Stopped |
| 任意 | 運転員pause | PauseRequested | 現状態を保持、新規配送禁止 |
| 任意 | 整合性違反・記録破損 | Fault（記録可能なら） | Reconcile |
| Reconcile | 実行状態を裏付ける照合結果＋必要な運転員指示 | Reconciled | 根拠に応じてRunning/ReportReady/Accepted/WaitTask/Stopped |

通常の次タスクは前報告IDを参照する。明示的なACKを別ツールで呼ぶ、または次タスク受付時に前報告を同時ACKする（同一永続イベントとして記録）案を採る。ACKと次タスクの間のクラッシュで報告を消失させない。ACKは受領の宣言であり、試験合格やコード内容の承認と同一視しない。

### 4.3 不変条件

- レーンごとに未完了のタスクは最大1件。WaitTaskまたは許可されたWaitDecision以外から新タスクを受けない。
- タスクIDが同じ・原文ハッシュが同じ再送は保存済み状態を返す。外部実行を追加しない。
- 同一IDで異なる原文は衝突として拒否し、状態と両者のハッシュを提示する。
- 運転停止・判断待ち・照合待ちで新規タスクを自動配送しない。
- ワーカーのターン／レーン／運転世代が一致しないイベントは現行状態へ適用しない。
- Relay Coreは入力イベントと既存状態だけで次状態・外部効果要求を決める。I/O・時計・乱数・LLMに直接依存しない。
- 外部副作用（Codexターン開始など）は意図を永続化してから実施する。成功が未確定のときに自動再送しない。

## 5. 永続化・重複防止・復旧

### 5.1 永続モデル [P]

最小構成としてレーンごとの連番イベントファイル方式を提案する。外部DBを前提にしない。データ領域は作業リポジトリ外に置く。

```text
state/
  tuff-tag/
    lane.lock
    events/
      00000000000000000001.json
      00000000000000000002.json
      ...
    tmp/
```

各イベントにはschema_version、lane_id、binding_epoch、seq、event_type、prev_event_sha256、event_body、event_sha256を含める。ハッシュ計算は`event_sha256`を除いた所定のバイト列に対して行う。実装時にバイト列定義とテストベクターを固定する（例：固定ヘッダ＋UTF-8のbody原文、長さ付き符号化）。時刻は監視用のメタデータで順序の正本にしない。タスク・報告原文は該当イベントに含め、途中断片の無制限保存を要求しない。

単一ライターがロックを保持した状態で、同一ファイルシステム上の一時ファイルへ書き、`sync_all`、閉じる、確定ファイルへ原子的に公開し、親ディレクトリを同期する。確定連番を上書きしない。既存連番・不正な世代・容量不足・同期失敗は停止して照合する。Linux実装の原子性・耐電源断性は対象ファイルシステムで確認する。他OSの同等保証は将来課題。

起動時は先頭から連続したイベントのスキーマ・連番・ハッシュを確認して状態を再生する。確定ファイルの欠損・破損を黙ってスキップしない。未確定tmpは隔離し、確定イベントとして扱わない。初版はスナップショット最適化を要求しない。ログが長大化した時の保存期間・圧縮・バックアップは運用実測後に協議する。

このログは改変検出補助であり、ローカル管理者からの改変を防ぐWORM保管を保証しない。初版でTUFF-DBを必須にしない。

### 5.2 不確定状態の扱い

| 障害位置 | 復旧時の処理 |
|---|---|
| 受付前 | 同じキーで受付可。ログに受付がなければ未受付。ただし外部送信は意図記録なしに行われない実装であること |
| 受付後・意図記録前 | 未配送と証明できるAcceptedから再開可能（運転許可が必要） |
| DispatchIntent後・turn ID保存前 | 自動再実行禁止。Codexスレッドの実行履歴を照合し、対応ターンの有無を確定 |
| Running中 | 公式接続の状態照会／再開で対応ターンを確認。内容の一部だけで成功判定しない |
| 報告保存後・監督ACK前 | 同一report IDで再提示。ワーカーは再実行しない。監督側に冪等なACKと次タスクIDの対応を要求 |
| ACK後・次タスク受付前 | WaitTaskから継続。監督側が停止している場合は状態表示、勝手に別会話を起動しない |
| ログ破損・書込み不能 | 新規外部効果を発行せず隔離。実行中ワーカーが存在し得ることを表示し、運転員が確認 |

外部システム間のトランザクションは存在しない。通信全体での厳密なexactly-once deliveryを宣言しない。Relayの受付冪等性と、送信不明時の照合・停止によって二重実行を防ぐ設計とする。監督側が報告を受け取ったか不明な場合、同一ID以外で再送しない。正式経路が同一性を保証できなければ運転員が照合する。

## 6. アダプタ契約

### 6.1 Core境界

```text
reduce(previous_state, event) -> (new_state, effect_intents)
Store.append(event) -> durable_sequence またはエラー
Worker.start_turn(binding, task) -> turn_id または不確定
Worker.observe(binding, turn_id) -> typed events
Worker.reconcile(binding, task_id, known_turn_id) -> evidence / unknown
Worker.interrupt(binding, turn_id) -> request accepted / uncertain
Supervisor.offer_report(binding, report) -> accepted / pending / unknown
```

実際のtraitは同期／非同期I/O方式を決めてから定義する。Coreに外部APIの型を直接持ち込まない。Adapterは不明な外部状態を独自推測で成功に変換しない。

### 6.2 Codex app-server Adapter [P/R-02]

公式のJSON-RPC・stdioを第一候補にする。現行バージョンとスキーマを実機で記録し、対応版を固定する。

1. レーン専用にapp-serverを起動し、`initialize`／`initialized`を正規手順で完了。
2. 対象の保存済みスレッドを`thread/resume`で再開し、ID・cwd・設定を確認。既存TUIが稼働中でないことを確認。
3. TaskAccepted・DispatchIntentの永続化後、`turn/start`で既存Task JSONをワーカーの入力として渡す。
4. 応答のturn IDを確定保存する。ID受領前の通知は対応を検証して一時保持し、未対応のまま捨てない。
5. `item/agentMessage/delta`等は進捗表示用。最終の完成済みメッセージ／必要なターン履歴を取得し、原文を保存する。
6. `turn/completed`通知を受けたらstatusを検査。completed、failed、interruptedを区別する。内容上の完了／判断要求は第3.3節に従う。
7. サーバーからの承認要求は種別・request ID・対象ターンと共に表示する。既存の承認方針を維持し、無条件の許可を自動送信しない。
8. 再接続時は保存済みスレッドの対応ターンを正式な照会で確認し、未確定を解消できない場合はReconcileを維持する。

`thread/read`などの履歴照会、`turn/interrupt`などの中断操作の具体的な引数は実機の生成スキーマで検証する。バージョン互換性がない場合は停止し、仕様を推測しない。認証は既存の公式Codex認証を利用する。認証ファイル・アクセストークンをログやGPTへ転送しない。

公式参考： https://developers.openai.com/codex/app-server

### 6.3 Supervisor Adapter [R-01/R-04]

第一候補は認証済みMCPツール。以下はRelay側ツール設計案であり、ChatGPTの実在する組み込みツールではない。監督は既存会話内でツールを使用する。

| Tool（案） | 要求 | 応答 |
|---|---|---|
| `relay_submit_task` | task key、前report ACK（必要時）、既存JSON原文 | 受付ID、現在状態。重複受付時は同一結果 |
| `relay_get_report` | 対応run/task ID | pendingまたは保存済みreport IDと原文、作業状態 |
| `relay_ack_report` | report ID | 受領記録の結果。同じIDの再ACKは冪等 |
| `relay_finish_run` | run ID、最終report ID | 正常終了。実行中は拒否 |
| `relay_status` | 接続スコープ内のレーン | 現在状態、最新時刻、必要な判断等 |

状態を変更するツールに偽のread-only注釈を付けない。チャット環境が都度確認を要求する場合、その確認を回避しない。get_reportは照会操作であり、新規作業や自動ACKを発生させない。

同期の短い往復が許容される場合はMCPの応答として報告を返せる。長時間タスクは受付と結果照会を分離し、MCP接続・呼出し制限に合わせて待機方式を決める。監督側が会話を終了した場合、Relayから当該会話を起動する公式手段が確認されない限り自動継続はできない。状態は保存したまま`監督待ち`と表示する。正常な次タスク発行をRelayが代行しない。

本番成立前に、(a)既存会話への接続、(b)レーン認証、(c)書込みツール権限、(d)連続呼出し、(e)長時間ターン完了後の再呼出し、(f)レート制限・確認画面、(g)切断復旧をPoCで確認する。使用するなら公式Secure MCP Tunnelでローカルに接続する。トンネルはRuntime API key等が必要な別コンポーネントであり、ChatGPTプランの推論費用と同一と仮定しない。

参考： https://developers.openai.com/api/docs/guides/secure-mcp-tunnels

### 6.4 模擬アダプタ（P1）

決められたTask JSONと固定報告を出すScripted Supervisor/WorkerをRustテスト内に置く。外部AIの使用・API課金なしで、正常往復、判断要求、クラッシュ、遅延、同一ID、異なるレーンを再現する。模擬接続の合格を実ChatGPTとの結合合格と呼ばない。

## 7. 運転員CLI [P/R-05]

コマンド名は提案。初版はCLIで完結し、ブラウザ拡張・GUIを必須にしない。管理コマンドはローカルの同一OSユーザーに限定する。常駐プロセスに管理要求を伝える場合はOSのローカルIPC（Unix domain socket等）を候補とし、ファイルの直接編集で状態を改変しない。

| コマンド（案） | 動作 |
|---|---|
| `tuff-relay doctor` | 設定、作業場所、接続機能、保存領域、アダプタ版を確認。タスク実行はしない |
| `tuff-relay run --lane tuff-tag` | 初回運転開始または登録済みレーンの起動 |
| `tuff-relay status [--json]` | 全レーンの状態・active task・待ち先・最終更新 |
| `tuff-relay watch --lane tuff-tag` | 進捗・状態変化の表示。非侵襲的な読取り |
| `tuff-relay pause --lane tuff-tag` | 新規配送を停止。実行中ターンは原則継続して結果保持 |
| `tuff-relay interrupt --lane tuff-tag` | 対応する実行中ターンに公式中断要求。成功は外部確認で確定 |
| `tuff-relay resume --lane tuff-tag` | 停止解除。照合・承認が未解決なら拒否 |
| `tuff-relay reconcile --lane tuff-tag` | 正式な照会結果・未確定事項・運転員が決定する情報を表示 |
| `tuff-relay export --lane tuff-tag --message <id>` | 原文をそのまま出力。状態は変更しない |
| `tuff-relay stop --all` | 全レーンの新規配送停止。実行中ターンの中断は別操作 |

正常時の画面例（実測値ではない）：

```text
LANE       PHASE          TASK          WAITING FOR      CONTROL
TUFF-Tag   RUNNING        task-0042     Codex turn       RUN
TUFF-OS    WAIT_TASK      -             Supervisor GPT   RUN
TUFF-DB    WAIT_DECISION  task-0018     Human decision   PAUSED
```

ステータスのJSONと人間向け表示を分離。色だけで状態を示さない。判断要求のサマリ、原文、どのタスクか、何が未決かを確認できる。Codex承認要求の応答方式は公式プロトコルに基づき決定する（R-02）。Relay自身は任意コマンドを承認・実行しない。

## 8. 障害分類と制御

| コード（案） | 事象 | 対応 |
|---|---|---|
| E_CONFIG | 設定・バインド不整合 | レーン起動拒否 |
| E_DUP_CONFLICT | 同一IDで異なる本文 | 拒否・照合待ち |
| E_LANE_MISMATCH | レーン・世代・スレッド不一致 | 拒否・記録 |
| E_BUSY | 稼働中に別タスク受付 | 拒否・現在IDを返す |
| E_PROTOCOL | 未対応スキーマ・不正JSON | 拒否。原文の自動修復禁止 |
| E_DISPATCH_UNKNOWN | ワーカー実行有無不明 | Reconcile、再配送禁止 |
| E_REPORT_UNKNOWN | 監督受領不明 | 同一IDで照会、次工程を勝手に起動しない |
| E_WORKER_FAILED | ワーカーが失敗と確定 | 原文報告・外部状態を監督へ返す |
| E_DECISION | 判断要求 | 監督へ返送、必要時人間待ち |
| E_APPROVAL | 外部権限承認要求 | 既存権限に従い待機・可視化 |
| E_STORAGE | 保存不良・容量不足 | 新規副作用停止、照合 |
| E_SUPERVISOR_WAIT | 監督側の呼出し終了・停止等 | 報告保持、待機表示 |
| E_AUTH / E_RATE | 認証切れ／利用制限 | 状態保持、公式の復旧・待機条件に従う |

通信リトライは原則として副作用のない状態照会と同じIDの冪等操作だけを対象とする。バックオフ・ハートビート・サイズ上限は実測に基づく設定値としてR-06で決定する。初版で短い固定の作業タイムアウトや自動キルを新設しない。無限に繰返す処理が見えたら人間が停止する。厳密な重複イベントは機械的に排除するが、似た内容を独断で同一タスクと認定しない。

## 9. セキュリティ・最小権限

- 一般ユーザー権限で動作。rootを要求しない。認証情報・状態領域のアクセス権を限定する（Linux初期案：ディレクトリ0700、ファイル0600）。
- 接続先、リポジトリ、スレッドは運転員が登録する。GPTや報告の本文から任意シェル引数・パス・URLを組み立てない。
- 報告本文は次工程の評価材料であり、Relayの命令ではない。制御命令は正式なツール／管理CLIの型付き入力からのみ受ける。
- 保存する原文は開発資料・機密コードを含み得る。外部のテレメトリへ送信しない。進捗表示とエラーに認証情報・生Cookie・APIキーを出さない。原文に偶発的な秘密が含まれた場合の取扱いは既存開発ポリシーに従い、無断で削除や本文改変しない。
- Relayの認証・レーン認可は監督側の認証スコープと一致させる。ローカルMCPサーバーは必要なツールだけ公開する。トンネル以外の公開待受・任意ファイル読取・任意コマンド実行は初版対象外。
- Codexのsandbox・approval・ネットワーク・Git操作の許可範囲を自動化の名目で拡大しない。
- RelayはGit commit、push、reset、clean、rollbackを直接実行しない。ワーカーによる既存運用上の許可範囲はそのまま維持する。

## 10. 試験仕様 [P]

### 10.1 P0：接続成立性

実際の開発PCで、バージョン・契約・ツール権限・既存会話／スレッド継続性を記録する。最初の運転開始以外に必要となった人間操作（送信、貼付、承認、次ターン起動）を数える。

| ID | 試験 | 合格条件 |
|---|---|---|
| P0-01 | ChatGPT既存会話で公式カスタムツール呼出し | 登録レーンのみの送信／受信を実行できる |
| P0-02 | Codex既存スレッドの再開 | 作業文脈とリポジトリ・権限が保持される |
| P0-03 | 3件の短い往復（無害な読み取り作業） | 開始後の通常転送・起動操作0件、対応ID一致 |
| P0-04 | 長めの1件、ツール応答終了または通信断 | 実用時間で継続可能か、または人間介入が必要か明確に観測 |
| P0-05 | 判断要求・正常終了 | 次タスクの不意な起動なし、報告可視化 |

P0-01〜03が未成立なら、監督側自動接続を完成扱いしない。P0-04で限界が出た場合はR-01/R-06へ差分を戻す。無人運転のために規約・承認を回避しない。

### 10.2 P1：Core単体・状態再生・故障注入

| ID | 試験 | 期待結果 |
|---|---|---|
| UT-01 | 有効なtaskを受領し報告ACK後に次task | 正常遷移・原文一致 |
| UT-02 | 同一ID・同一本文を100回再送（試験値） | 外部実行効果1件以下、受付結果同一 |
| UT-03 | 同一ID・異なる本文 | 拒否、既存原文保持 |
| UT-04 | 不正JSON・不正UTF-8・未知版 | 受付拒否、状態変更なし |
| UT-05 | 別レーン／旧世代／別turnイベント | 拒否、現行状態変化なし |
| UT-06 | 判断要求・pause・resume | 新規配送を適切に抑止、承認後のみ復帰 |
| UT-07 | turn completedだが判断要求 | 通信完了を作業完了と扱わない |
| FT-01 | 受付記録前／後にクラッシュ | イベント再生、外部副作用の規定遵守 |
| FT-02 | DispatchIntent直後・応答喪失・turn保存前 | Reconcile、自動再送0件 |
| FT-03 | 実行中切断・再接続 | 対応ターンの照合、未確認成功なし |
| FT-04 | 報告保存後・ACK前にクラッシュ | 同一report ID再提示、ワーカー再実行0件 |
| FT-05 | ACKと次taskの受付間でクラッシュ | ACK／次taskの整合、報告消失なし |
| FT-06 | ジャーナル破損・連番欠損・容量不足 | 自動進行停止、破損を黙認しない |
| FT-07 | 手動pauseと外部完了が競合 | 結果保存、新規配送なし |
| FT-08 | 中断要求後に遅れて完了通知 | 実測された最終状態を保存。勝手な巻戻しなし |
| IT-01 | 2レーン並行、一方で判断待ち | 他方が独立継続 |
| IT-02 | 日本語・コードフェンス・改行を含む実例 | 監督→Codex→監督で指定原文を変更しない |
| IT-03 | 未登録の管理コマンド・権限昇格要求 | 拒否、権限維持 |
| IT-04 | 同じrepoへの二重起動 | 2番目を拒否 |

`cargo fmt --check`、`cargo clippy`、`cargo test`を基本とする。具体的なworkspace引数とLint設定は採用crate・既存規約に従う。テストは成功した実行証跡を保存し、未実行をPASSにしない。故障注入は副作用を伴う実開発リポジトリではなく模擬アダプタ／試験用作業ツリーで行う。

### 10.3 P2〜P4：実機統合

公式接続を固定し、最初は無害な読取り／テストだけを既存監督・ワーカーで3往復する。続いて監督の既決Task JSONを1件実際に処理する。ログ、原文ハッシュ、ID、完了通知、実際のテスト報告、介入回数を記録する。複数レーンの停止独立性も試験する。

完成判定は基本設計AC-01〜08を使用。模擬試験、短時間PoC、長時間実運用を区別する。初版の設計・実装に未決事項を含めない。判断が必要な部分だけ止め、独立した既決作業は継続する。

## 11. 未決事項と実装境界

| R-ID | 影響する詳細設計 | 未決でも進められる範囲 |
|---|---|---|
| R-01 | Supervisor Adapter・無人継続・認証 | Core、模擬監督、永続記録、運転CLI |
| R-02 | Codex Adapter・TUI切替・承認操作 | 模擬Worker・状態遷移・重複防止 |
| R-03 | WaitDecisionからの許可入力 | 判断要求を記録し未承認継続を禁止する処理 |
| R-04 | 現行Task JSONのID、報告分類・ACK形式 | 原文保持・封筒・模擬テスト。既存形式の変更は保留 |
| R-05 | 運転CLIの表示と通知・常駐方法 | 状態照会API、管理権限境界 |
| R-06 | タイムアウト・サイズ・認証更新・費用・保存期間 | 不確定状態モデル、容量不足停止、設定の拡張点 |

## 12. 初回Codexへの実装委託範囲（確定後）

1. P1 CoreのRust型、純粋なreducer、レーンIDと原文の検証、模擬アダプタ。
2. 永続イベント記録、再生、冪等受付、故障注入試験。
3. ローカルCLIのstatus/pause/resume/exportなどの最小操作。
4. 結果を監督GPTへ報告し、未決事項を列挙する。

この段階では、実ChatGPT接続、既存Codexセッション移行、認証情報の取得、ブラウザ操作、課金設定の変更を作業指示に含めない。P0は並行して実機で観測する。正式な接続方式が確定してからアダプタを実装する。

