
#[tokio::test]
async fn test_bedrock_document_s3() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config(NOVA_LITE);

    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Document(DocumentContent {
                source: MediaSource::S3(S3Location {
                    uri: "s3://test-bucket/doc.txt".to_string(),
                    bucket_owner: None,
                }),
                format: DocumentFormat::Txt,
                name: Some("s3-doc".to_string()),
            }),
            ContentBlock::text("Summarise this document in one sentence.".to_string()),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    assert!(!resp.text().is_empty());
}

// ---------------------------------------------------------------------------
// AWS Bedrock — multimodal: video
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bedrock_video_embedded() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config(NOVA_LITE);

    // Use tiny fake video bytes
    let fake_video_bytes = vec![0u8; 32];
    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Video(VideoContent {
                source: MediaSource::from_bytes("video/mp4", &fake_video_bytes),
                format: VideoFormat::Mp4,
            }),
            ContentBlock::text(
                "Describe the main subject of this video in one sentence.".to_string(),
            ),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_video_s3() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config(NOVA_LITE);

    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Video(VideoContent {
                source: MediaSource::S3(S3Location {
                    uri: "s3://test-bucket/video.mp4".to_string(),
                    bucket_owner: None,
                }),
                format: VideoFormat::Mp4,
            }),
            ContentBlock::text("What is the main subject of this video? One sentence.".to_string()),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}

// ---------------------------------------------------------------------------
// AWS Bedrock — mixed modalities
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bedrock_image_and_document() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config(NOVA_LITE);

    let doc = b"Context: The image shows a white pixel on a white background.";
    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Image(ImageContent {
                source: MediaSource::base64("image/png", TINY_PNG_B64),
                format: Some(ImageFormat::Png),
                detail: None,
            }),
            ContentBlock::Document(DocumentContent {
                source: MediaSource::from_bytes("text/plain", doc),
                format: DocumentFormat::Txt,
                name: Some("context".to_string()),
            }),
            ContentBlock::text(
                "According to the document, what color does the image show? One word.".to_string(),
            ),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}

// ---------------------------------------------------------------------------
// AWS Bedrock — prompt caching
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bedrock_prompt_caching_system() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config(NOVA_LITE);

    let system_msg = Message {
        role: Role::System,
        content: vec![ContentBlock::text(
            "You are a helpful assistant.".to_string(),
        )],
        name: None,
        cache_control: Some(sideseat::CacheControl::Ephemeral),
    };

    let resp = provider
        .complete(
            vec![system_msg, user_msg("Reply with the word 'ok'.")],
            config,
        )
        .await
        .unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_prompt_caching_message() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config(NOVA_LITE);

    let cached_msg = Message {
        role: Role::User,
        content: vec![ContentBlock::text(
            "The sky is blue. The grass is green.".to_string(),
        )],
        name: None,
        cache_control: Some(sideseat::CacheControl::Ephemeral),
    };

    let resp = provider
        .complete(vec![cached_msg, user_msg("What color is the sky?")], config)
        .await
        .unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_audio_converse_unsupported() {
    // Docs: audio input is NOT supported via the Converse API.
    // The SDK rejects ContentBlock::Audio before making a network call.
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(400)
            .header("content-type", "application/json")
            .body(BEDROCK_VALIDATION_ERROR_JSON);
    });
    let config = default_config(NOVA_LITE);

    let result = provider
        .complete(
            vec![Message {
                role: Role::User,
                content: vec![
                    ContentBlock::Audio(AudioContent {
                        source: MediaSource::from_bytes("audio/mp3", &[0u8; 16]),
                        format: AudioFormat::Mp3,
                    }),
                    ContentBlock::text("Transcribe this.".to_string()),
                ],
                name: None,
                cache_control: None,
            }],
            config,
        )
        .await;

    assert!(
        matches!(result, Err(ProviderError::Unsupported(_))),
        "audio via Converse API should return Unsupported, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// AWS Bedrock API key — basic chat (same provider, different construction)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bedrock_api_key_complete() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config(NOVA_LITE);

    let resp = provider
        .complete(vec![user_msg("Say 'hello' in one word")], config)
        .await
        .unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_api_key_stream() {
    let (server, provider) = mock_bedrock();
    let body = bedrock_converse_stream_body();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse-stream$");
        then.status(200)
            .header("content-type", "application/vnd.amazon.eventstream")
            .body(body);
    });
    let config = default_config(NOVA_LITE);

    let stream = provider.stream(vec![user_msg("Count from 1 to 3")], config);
    let resp = collect_stream(stream).await.unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
    assert!(resp.usage.output_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_api_key_tools() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_TOOL_JSON);
    });
    let mut config = default_config(HAIKU);
    config.tools = vec![echo_tool()];

    let resp = provider
        .complete(vec![user_msg("Please echo the word 'jackfruit'")], config)
        .await
        .unwrap();

    let has_tool = resp
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(has_tool, "expected tool_use, got: {:?}", resp.content);
}

#[tokio::test]
async fn test_bedrock_api_key_system_prompt() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let mut config = default_config(NOVA_LITE);
    config.system = Some("You are a pirate. Always respond like a pirate.".to_string());

    let resp = provider
        .complete(vec![user_msg("Greet me")], config)
        .await
        .unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_api_key_streaming_tools() {
    let (server, provider) = mock_bedrock();
    let body = bedrock_converse_stream_tool_body();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse-stream$");
        then.status(200)
            .header("content-type", "application/vnd.amazon.eventstream")
            .body(body);
    });
    let mut config = default_config(HAIKU);
    config.tools = vec![echo_tool()];

    let stream = provider.stream(vec![user_msg("Please echo the word 'mango'")], config);
    let resp = collect_stream(stream).await.unwrap();

    let has_tool = resp
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse(_)));
    assert!(
        has_tool,
        "expected tool_use in stream, got: {:?}",
        resp.content
    );
}

#[tokio::test]
async fn test_bedrock_api_key_list_models() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(GET).path_includes("foundation-models");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_LIST_MODELS_JSON);
    });

    let models = provider.list_models().await.unwrap();
    assert!(!models.is_empty(), "should return at least one model");
    assert!(
        models
            .iter()
            .any(|m| m.id.contains("amazon.nova") || m.id.contains("anthropic.claude")),
        "expected a Nova or Claude model, got: {:?}",
        models.iter().map(|m| &m.id).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_bedrock_api_key_embed_titan() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/invoke$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_EMBED_TITAN_JSON);
    });

    let req = EmbeddingRequest::new(
        "amazon.titan-embed-text-v2:0",
        vec!["Hello world", "Goodbye world"],
    )
    .with_dimensions(256);
    let resp = provider.embed(req).await.unwrap();

    assert_eq!(
        resp.embeddings.len(),
        1,
        "Titan Embed returns one vector per call"
    );
    assert!(!resp.embeddings[0].is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_api_key_embed_titan_v2_dims() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/invoke$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_EMBED_TITAN_JSON);
    });
    let model = "amazon.titan-embed-text-v2:0";

    for &dims in &[256u32, 512, 1024] {
        let req = EmbeddingRequest::new(model, vec!["The quick brown fox"]).with_dimensions(dims);
        let resp = provider
            .embed(req)
            .await
            .unwrap_or_else(|e| panic!("titan-embed-text-v2 dims={dims} failed: {e:?}"));
        assert_eq!(resp.embeddings.len(), 1);
        assert!(!resp.embeddings[0].is_empty(), "dims={dims}");
        assert!(resp.usage.input_tokens > 0);
    }
}

#[tokio::test]
async fn test_bedrock_api_key_embed_titan_v1() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/invoke$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_EMBED_TITAN_JSON);
    });

    let req = EmbeddingRequest::new("amazon.titan-embed-text-v1:0", vec!["The quick brown fox"]);
    let resp = provider.embed(req).await.unwrap();

    assert_eq!(resp.embeddings.len(), 1);
    assert!(!resp.embeddings[0].is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_api_key_embed_titan_multimodal() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/invoke$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_EMBED_TITAN_MULTIMODAL_JSON);
    });

    for &dims in &[256u32, 384, 1024] {
        let req = EmbeddingRequest::new(
            "amazon.titan-embed-image-v1:0",
            vec!["A serene mountain lake"],
        )
        .with_dimensions(dims);
        let resp = provider
            .embed(req)
            .await
            .unwrap_or_else(|e| panic!("titan-embed-image dims={dims} failed: {e:?}"));
        assert_eq!(resp.embeddings.len(), 1);
        assert!(!resp.embeddings[0].is_empty(), "dims={dims}");
    }
}

#[tokio::test]
async fn test_bedrock_api_key_embed_cohere_english() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/invoke$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_EMBED_COHERE_JSON);
    });

    let req = EmbeddingRequest::new(
        "cohere.embed-english-v3",
        vec!["Hello world", "Goodbye world"],
    );
    let resp = provider.embed(req).await.unwrap();

    assert!(
        !resp.embeddings.is_empty(),
        "expected at least one embedding"
    );
    assert!(!resp.embeddings[0].is_empty());
}

#[tokio::test]
async fn test_bedrock_api_key_embed_cohere_multilingual() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/invoke$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_EMBED_COHERE_JSON);
    });

    let req = EmbeddingRequest::new(
        "cohere.embed-multilingual-v3",
        vec!["Hello", "Bonjour", "Hola"],
    );
    let resp = provider.embed(req).await.unwrap();

    assert!(
        !resp.embeddings.is_empty(),
        "expected at least one embedding"
    );
    assert!(!resp.embeddings[0].is_empty());
}

#[tokio::test]
async fn test_bedrock_api_key_generate_image() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/invoke$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_IMAGE_GEN_JSON);
    });

    let req = ImageGenerationRequest::new(
        "amazon.nova-canvas-v1:0",
        "a red circle on a white background",
    )
    .with_size(ImageSize::S512x512);
    let resp = provider.generate_image(req).await.unwrap();

    assert_eq!(resp.images.len(), 1, "expected one image");
    assert!(
        resp.images[0].b64_json.is_some(),
        "expected b64_json in response"
    );
}

#[tokio::test]
async fn test_bedrock_api_key_generate_video_requires_s3() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_includes("async-invoke");
        then.status(400)
            .header("content-type", "application/json")
            .body(BEDROCK_ASYNC_INVOKE_ERROR_JSON);
    });

    let req = VideoGenerationRequest::new("amazon.nova-reel-v1:0", "a cat walking")
        .with_output_storage_uri("s3://nonexistent-bucket-sideseat-test/output/");
    let result = provider.generate_video(req).await;

    match result {
        Ok(_) => panic!("expected error with fake S3 bucket"),
        Err(ProviderError::Api { status, .. }) => {
            assert!(
                status == 400 || status == 403 || status == 500 || status == 0,
                "unexpected status: {status}"
            );
        }
        Err(e) => panic!("unexpected error type: {e:?}"),
    }
}

#[tokio::test]
async fn test_bedrock_api_key_count_tokens() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_includes("count-tokens");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COUNT_TOKENS_JSON);
    });
    let config = default_config(NOVA_LITE);

    match provider
        .count_tokens(vec![user_msg("Hello, world!")], config)
        .await
    {
        Ok(count) => assert!(count.input_tokens > 0, "expected > 0 input tokens"),
        Err(e) if bedrock_model_not_available(&e) => {}
        Err(e) => panic!("count_tokens failed: {e:?}"),
    }
}

#[tokio::test]
async fn test_bedrock_api_key_response_model_populated() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config(NOVA_LITE);

    let resp = provider
        .complete(vec![user_msg("Say 'hi'")], config)
        .await
        .unwrap();

    assert!(
        resp.model.is_some(),
        "resp.model should be populated; got None"
    );
}

#[tokio::test]
async fn test_bedrock_api_key_multi_turn_tool_use() {
    let (server, provider) = mock_bedrock();
    // Turn 2: body has toolResult → text (registered FIRST = checked first)
    server.mock(|when, then| {
        when.method(POST)
            .path_matches(r".*/converse$")
            .body_includes("toolResult");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    // Turn 1 fallback: no restriction → tool_use (checked second)
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_TOOL_JSON);
    });
    let mut config = default_config(HAIKU);
    config.tools = vec![echo_tool()];

    let turn1_msgs = vec![user_msg("Please echo the word 'jackfruit'")];
    let resp1 = provider
        .complete(turn1_msgs.clone(), config.clone())
        .await
        .unwrap();

    let tool_use = resp1
        .content
        .iter()
        .find_map(|b| {
            if let ContentBlock::ToolUse(t) = b {
                Some(t.clone())
            } else {
                None
            }
        })
        .expect("expected tool_use in turn 1 response");

    let turn2_msgs = vec![
        user_msg("Please echo the word 'jackfruit'"),
        Message {
            role: Role::Assistant,
            content: resp1.content.clone(),
            name: None,
            cache_control: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult(ToolResultBlock {
                tool_use_id: tool_use.id.clone(),
                content: vec![ContentBlock::text("jackfruit".to_string())],
                is_error: false,
            })],
            name: None,
            cache_control: None,
        },
    ];
    let resp2 = provider.complete(turn2_msgs, config).await.unwrap();

    let has_text = resp2
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::Text(_)));
    assert!(
        has_text,
        "final response should contain text, got: {:?}",
        resp2.content
    );
}

#[tokio::test]
async fn test_bedrock_api_key_vision() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config(NOVA_LITE);

    let resp = provider
        .complete(
            vec![vision_message("Describe what you see in one word.")],
            config,
        )
        .await
        .unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_api_key_stop_reason_max_tokens() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_MAX_TOKENS_JSON);
    });
    let mut config = default_config(NOVA_LITE);
    config.max_tokens = Some(1);

    let resp = provider
        .complete(vec![user_msg("Count from 1 to 100")], config)
        .await
        .unwrap();

    assert_eq!(
        resp.stop_reason,
        StopReason::MaxTokens,
        "expected MaxTokens stop reason, got: {:?}",
        resp.stop_reason
    );
}

#[tokio::test]
async fn test_bedrock_api_key_multi_image() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config(NOVA_LITE);

    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Image(ImageContent {
                source: MediaSource::base64("image/png", TINY_PNG_B64),
                format: Some(ImageFormat::Png),
                detail: None,
            }),
            ContentBlock::Image(ImageContent {
                source: MediaSource::base64("image/png", TINY_PNG_B64),
                format: Some(ImageFormat::Png),
                detail: None,
            }),
            ContentBlock::text("How many images are shown? Reply with just a number.".to_string()),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_bedrock_api_key_document_txt() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config(NOVA_LITE);

    let doc = b"The capital of Japan is Tokyo. The population is approximately 14 million.";
    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Document(DocumentContent {
                source: MediaSource::from_bytes("text/plain", doc),
                format: DocumentFormat::Txt,
                name: Some("facts".to_string()),
            }),
            ContentBlock::text("What is the capital of Japan? Answer in one word.".to_string()),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    assert!(!resp.text().is_empty());
}

#[tokio::test]
async fn test_bedrock_api_key_image_and_document() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config(NOVA_LITE);

    let doc = b"Hint: The image shows a single white pixel.";
    let msg = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Image(ImageContent {
                source: MediaSource::base64("image/png", TINY_PNG_B64),
                format: Some(ImageFormat::Png),
                detail: None,
            }),
            ContentBlock::Document(DocumentContent {
                source: MediaSource::from_bytes("text/plain", doc),
                format: DocumentFormat::Txt,
                name: Some("hint".to_string()),
            }),
            ContentBlock::text(
                "According to the document, what does the image show? One word.".to_string(),
            ),
        ],
        name: None,
        cache_control: None,
    };

    let resp = provider.complete(vec![msg], config).await.unwrap();
    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}

#[tokio::test]
async fn test_bedrock_api_key_prompt_caching() {
    let (server, provider) = mock_bedrock();
    server.mock(|when, then| {
        when.method(POST).path_matches(r".*/converse$");
        then.status(200)
            .header("content-type", "application/json")
            .body(BEDROCK_COMPLETE_JSON);
    });
    let config = default_config(NOVA_LITE);

    let cached_msg = Message {
        role: Role::User,
        content: vec![ContentBlock::text(
            "The capital of France is Paris.".to_string(),
        )],
        name: None,
        cache_control: Some(sideseat::CacheControl::Ephemeral),
    };

    let resp = provider
        .complete(
            vec![cached_msg, user_msg("What is the capital of France?")],
            config,
        )
        .await
        .unwrap();

    assert!(!resp.text().is_empty());
    assert!(resp.usage.input_tokens > 0);
}
