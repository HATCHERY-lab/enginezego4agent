use ez4a_harness::{usage_doc, Generation, TurnInput, TurnKind, RESTART_CAP};
use ez4a_llm::{FauxLlm, ToolCall, ToolSchema, Usage};
use ez4a_session::{
    Session, SubmissionStatus, TaskState, ROOT_CONVERSATION_ID,
};
use ez4a_types::TaskStatus;
use serde_json::json;

fn usage(input: i64, output: i64) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        provider: "faux".into(),
        model: "scripted".into(),
        cost_usd: 0.0,
    }
}

fn input(text: &str, tools: Vec<ToolSchema>) -> TurnInput {
    TurnInput {
        conversation_id: ROOT_CONVERSATION_ID,
        user_text: text.into(),
        tools,
    }
}

#[tokio::test]
async fn high_usage_writes_compaction_and_the_next_request_drops_the_old_text() {
    let session = Session::memory().await.unwrap();
    let llm = FauxLlm::text("OLD")
        .with_usage(usage(200_000, 1))
        .then(FauxLlm::text("SUM"));
    let outcome = Generation::turn(&session, &llm, &input("remember OLD", Vec::new()))
        .await
        .unwrap();
    assert!(matches!(outcome.kind, TurnKind::Answered { .. }));
    assert_eq!(llm.calls(), 2, "the compressor must be called");
    let view = session.watch(ROOT_CONVERSATION_ID).await.unwrap();
    assert!(
        view.borrow()
            .entries
            .iter()
            .any(|entry| entry.kind == "pi.compaction"),
        "journal missing pi.compaction"
    );

    let next = FauxLlm::text("ok");
    Generation::turn(&session, &next, &input("second", Vec::new()))
        .await
        .unwrap();
    let request = &next.requests()[0];
    let blob = format!("{request:?}");
    assert!(!blob.contains("remember OLD"), "{blob}");
    assert!(blob.contains("SUM"), "{blob}");
    session.close().await.unwrap();
}

#[tokio::test]
async fn next_request_drops_a_tool_call_that_never_returned() {
    let session = Session::memory().await.unwrap();
    let llm = FauxLlm::text("").with_tool_call(ToolCall {
        id: "c1".into(),
        name: "powershell".into(),
        arguments: json!({"command": "Start-Sleep -Seconds 25"}),
    });
    let outcome = Generation::turn(
        &session,
        &llm,
        &input(
            "sleep",
            vec![ToolSchema {
                name: "powershell".into(),
                description: "shell".into(),
                parameters: json!({"type": "object"}),
            }],
        ),
    )
    .await
    .unwrap();
    assert!(matches!(outcome.kind, TurnKind::WaitingTool { .. }));
    let next = FauxLlm::text("ok");
    Generation::turn(&session, &next, &input("read note", Vec::new()))
        .await
        .unwrap();
    let blob = format!("{:?}", next.requests()[0]);
    assert!(!blob.contains("Start-Sleep"), "{blob}");
    assert!(blob.contains("read note"), "{blob}");
    session.close().await.unwrap();
}

#[tokio::test]
async fn text_turn_publishes_assistant_and_usage() {
    let session = Session::memory().await.unwrap();
    let llm = FauxLlm::text("hello").with_usage(usage(11, 7));
    let outcome = Generation::turn(&session, &llm, &input("ping", Vec::new()))
        .await
        .unwrap();
    let TurnKind::Answered { entry_id } = outcome.kind else {
        panic!("text turn should answer");
    };
    assert!(outcome.called_llm);
    assert_eq!(outcome.failed_attempts, 0);
    assert_eq!(llm.calls(), 1);
    let stored = session.entry(entry_id).await.unwrap().unwrap();
    assert_eq!(stored.kind, "pi.assistant");
    assert_eq!(stored.model.as_ref().unwrap()[0]["content"][0]["text"], json!("hello"));
    assert_eq!(stored.model.as_ref().unwrap()[0]["usage"]["input"], json!(11));
    assert_eq!(stored.model.as_ref().unwrap()[0]["usage"]["output"], json!(7));
    let ledger = session
        .snapshot_doc(&usage_doc(), Some(ROOT_CONVERSATION_ID))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ledger["models"]["faux/scripted"]["input"], json!(11));
    assert_eq!(ledger["models"]["faux/scripted"]["output"], json!(7));
    assert_eq!(ledger["models"]["faux/scripted"]["cost"], json!(0));
    let task = session.task(outcome.task_id).await.unwrap().unwrap();
    assert!(task.restarts.is_none());
    assert_eq!(task.state.status(), TaskStatus::Terminal);
    session.close().await.unwrap();
}

#[tokio::test]
async fn tool_call_turn_publishes_waiting_tool_task() {
    let session = Session::memory().await.unwrap();
    let llm = FauxLlm::text("")
        .with_tool_call(ToolCall {
            id: "call-1".into(),
            name: "read".into(),
            arguments: json!({ "path": "note.txt" }),
        })
        .with_usage(usage(4, 1));
    let tools = vec![ToolSchema {
        name: "read".into(),
        description: "Read a file".into(),
        parameters: json!({ "type": "object", "required": ["path"] }),
    }];
    let generation = Generation::commit_intent(&session, &input("read note.txt", tools))
        .await
        .unwrap();
    let outcome = generation.respond(&session, &llm).await.unwrap();
    let TurnKind::WaitingTool {
        entry_id,
        tool_task_id,
    } = outcome.kind
    else {
        panic!("tool turn should wait");
    };
    let tool = session.task(tool_task_id).await.unwrap().unwrap();
    assert_eq!(tool.kind, "pi.tool");
    assert_eq!(tool.state.status(), TaskStatus::Waiting);
    assert_eq!(tool.owner, Some(generation.task_id()));
    assert!(tool.ended_at.is_none());
    assert_eq!(tool.input["callId"], json!("call-1"));
    assert_eq!(tool.input["kind"], json!("model"));
    let parent = session.task(generation.task_id()).await.unwrap().unwrap();
    assert_eq!(parent.state.status(), TaskStatus::Waiting);
    let view = session.watch(ROOT_CONVERSATION_ID).await.unwrap();
    assert!(view
        .borrow()
        .entries
        .iter()
        .any(|entry| entry.id == entry_id && entry.kind == "pi.assistant"));
    assert!(view
        .borrow()
        .entries
        .iter()
        .all(|entry| entry.kind != "pi.tool-result"));
    let again = generation.resume(&session, &llm).await.unwrap();
    assert!(!again.called_llm);
    assert_eq!(llm.calls(), 1);
    assert_eq!(
        session.task(tool_task_id).await.unwrap().unwrap().state.status(),
        TaskStatus::Waiting
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn watch_and_snapshot_see_the_entry_only_after_commit() {
    let session = Session::memory().await.unwrap();
    let llm = FauxLlm::text("after").with_usage(usage(3, 2));
    let generation = Generation::commit_intent(&session, &input("before", Vec::new()))
        .await
        .unwrap();
    let mut watch = session.watch(ROOT_CONVERSATION_ID).await.unwrap();
    assert_eq!(watch.borrow().entries.len(), 1);
    assert_eq!(watch.borrow().entries[0].kind, "pi.user");
    assert!(watch
        .borrow()
        .entries
        .iter()
        .all(|entry| entry.kind != "pi.assistant"));
    assert!(session
        .snapshot_doc(&usage_doc(), Some(ROOT_CONVERSATION_ID))
        .await
        .unwrap()
        .is_none());
    let outcome = generation.respond(&session, &llm).await.unwrap();
    let entry_id = outcome.kind.entry_id().unwrap();
    watch.changed().await.unwrap();
    let view = watch.borrow().clone();
    assert!(view
        .entries
        .iter()
        .any(|entry| entry.id == entry_id && entry.kind == "pi.assistant"));
    assert_eq!(
        view.docs.get("pi.usage").unwrap()["models"]["faux/scripted"]["output"],
        json!(2)
    );
    let stored = session.entry(entry_id).await.unwrap();
    assert_eq!(stored.unwrap().kind, "pi.assistant");
    let ledger = session
        .snapshot_doc(&usage_doc(), Some(ROOT_CONVERSATION_ID))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ledger["models"]["faux/scripted"]["input"], json!(3));
    session.close().await.unwrap();
}

#[tokio::test]
async fn resume_after_intent_failure_cap() {
    let session = Session::memory().await.unwrap();
    let llm = FauxLlm::text("retried").with_usage(usage(1, 1));
    let generation = Generation::commit_intent(&session, &input("again", Vec::new()))
        .await
        .unwrap();
    let parked = session.task(generation.task_id()).await.unwrap().unwrap();
    assert!(matches!(
        &parked.state,
        TaskState::Running { checkpoint } if checkpoint["phase"] == "request"
    ));
    assert!(parked.restarts.is_none());
    let submission_id = parked.input["submissionId"].as_i64().unwrap();
    let submission = session.submission(submission_id).await.unwrap().unwrap();
    assert_eq!(submission.status, SubmissionStatus::Placed);
    assert!(session
        .watch(ROOT_CONVERSATION_ID)
        .await
        .unwrap()
        .borrow()
        .entries
        .iter()
        .all(|entry| entry.kind != "pi.assistant"));

    let first = generation.resume(&session, &llm).await.unwrap();
    assert!(first.called_llm);
    assert_eq!(first.failed_attempts, 1);
    assert_eq!(llm.calls(), 1);
    assert!(matches!(first.kind, TurnKind::Answered { .. }));
    assert_eq!(
        session.task(generation.task_id()).await.unwrap().unwrap().restarts,
        Some(1)
    );
    let second = generation.resume(&session, &llm).await.unwrap();
    assert!(!second.called_llm);
    assert_eq!(llm.calls(), 1);
    session.close().await.unwrap();

    let session = Session::memory().await.unwrap();
    let llm = FauxLlm::failing("down");
    let generation = Generation::commit_intent(&session, &input("crash", Vec::new()))
        .await
        .unwrap();
    for expected in 1..RESTART_CAP {
        let error = generation.resume(&session, &llm).await.unwrap_err();
        assert_eq!(error.to_string(), "down");
        let task = session.task(generation.task_id()).await.unwrap().unwrap();
        assert_eq!(task.restarts, Some(expected));
        assert_eq!(task.state.status(), TaskStatus::Running);
        assert_eq!(llm.calls(), expected as u64);
    }
    let calls = llm.calls();
    let capped = generation.resume(&session, &llm).await.unwrap();
    assert!(!capped.called_llm);
    assert_eq!(capped.failed_attempts, RESTART_CAP);
    assert_eq!(capped.kind, TurnKind::RestartLimit);
    assert_eq!(llm.calls(), calls);
    let again = generation.resume(&session, &llm).await.unwrap();
    assert!(!again.called_llm);
    assert_eq!(again.kind, TurnKind::RestartLimit);
    assert_eq!(llm.calls(), calls);
    assert_eq!(
        session.task(generation.task_id()).await.unwrap().unwrap().state.status(),
        TaskStatus::Terminal
    );
    session.close().await.unwrap();
}
