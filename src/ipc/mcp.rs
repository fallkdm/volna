use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Запуск MCP (Model Context Protocol) сервера по протоколу JSON-RPC 2.0 через stdio
pub async fn run_mcp_server() {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(stdin).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let req: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[mcp] Ошибка разбора JSON: {e}");
                let err_resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32700,
                        "message": format!("Parse error: {}", e)
                    }
                });
                let _ = send_json_rpc(&mut stdout, &err_resp).await;
                continue;
            }
        };

        if let Some(resp) = handle_json_rpc(req).await {
            let _ = send_json_rpc(&mut stdout, &resp).await;
        }
    }
}

async fn send_json_rpc<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    val: &serde_json::Value,
) -> Result<(), std::io::Error> {
    let mut out = serde_json::to_string(val).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    out.push('\n');
    writer.write_all(out.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

/// Обработка одного входящего JSON-RPC запроса
pub async fn handle_json_rpc(req: serde_json::Value) -> Option<serde_json::Value> {
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = req.get("id").cloned();

    // Уведомления не требуют ответа
    if method == "notifications/initialized" || (method.starts_with("notifications/") && id.is_none()) {
        return None;
    }

    match method {
        "initialize" => {
            Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "volna-mcp",
                        "version": "1.0.0"
                    }
                }
            }))
        }
        "ping" => {
            Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {}
            }))
        }
        "tools/list" => {
            Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": get_tool_definitions()
                }
            }))
        }
        "tools/call" => {
            let params = req.get("params");
            let tool_name = params.and_then(|p| p.get("name")).and_then(|n| n.as_str()).unwrap_or("");
            let args = params.and_then(|p| p.get("arguments")).cloned().unwrap_or(serde_json::json!({}));

            let result = execute_tool_call(tool_name, &args).await;
            Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result
            }))
        }
        _ => {
            // Для неизвестных методов возвращаем ошибку MethodNotFound только если есть id
            if let Some(id) = id {
                Some(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32601,
                        "message": format!("Method '{}' not found", method)
                    }
                }))
            } else {
                None
            }
        }
    }
}

/// Список определений инструментов MCP
pub fn get_tool_definitions() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "volna_status",
            "description": "Получить статус плеера Волна (играет ли, текущий трек, артист, альбом, таймкод, активный провайдер)",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
        serde_json::json!({
            "name": "volna_control",
            "description": "Управление воспроизведением плеера",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["play", "pause", "toggle", "next", "prev"]
                    }
                },
                "required": ["action"]
            }
        }),
        serde_json::json!({
            "name": "volna_volume",
            "description": "Установка громкости (0.0 - 1.0)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "volume": {
                        "type": "number",
                        "minimum": 0.0,
                        "maximum": 1.0
                    }
                },
                "required": ["volume"]
            }
        }),
        serde_json::json!({
            "name": "volna_switch_provider",
            "description": "Переключение музыкального сервиса (yandex, ytmusic или vk)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "provider": {
                        "type": "string",
                        "enum": ["yandex", "ytmusic", "vk"]
                    }
                },
                "required": ["provider"]
            }
        }),
        serde_json::json!({
            "name": "volna_start_wave",
            "description": "Запуск персональной волны / радио",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "station_id": {
                        "type": "string"
                    }
                }
            }
        }),
        serde_json::json!({
            "name": "volna_wave_filter",
            "description": "Применить фильтры Волны. Язык: russian, not-russian, without-words. Характер: favorite, discover, popular. Настроение (только Яндекс): active, calm, fun, sad. Без параметров — полный сброс фильтров",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "language": {
                        "type": "string",
                        "enum": ["russian", "not-russian", "without-words"]
                    },
                    "character": {
                        "type": "string",
                        "enum": ["favorite", "discover", "popular"]
                    },
                    "mood": {
                        "type": "string",
                        "enum": ["active", "calm", "fun", "sad"]
                    }
                }
            }
        }),
        serde_json::json!({
            "name": "volna_search_and_play",
            "description": "Поиск и воспроизведение трека по запросу",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string"
                    }
                },
                "required": ["query"]
            }
        }),
        serde_json::json!({
            "name": "volna_set_cookie",
            "description": "Установка куки YouTube Music без диалогов в UI",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cookie": {
                        "type": "string"
                    }
                },
                "required": ["cookie"]
            }
        }),
        serde_json::json!({
            "name": "volna_like",
            "description": "Поставить или переключить лайк текущему треку",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "liked": {
                        "type": "boolean",
                        "description": "true для установки лайка, false для снятия, или опустить для переключения"
                    }
                }
            }
        }),
        serde_json::json!({
            "name": "volna_dislike",
            "description": "Поставить дизлайк текущему треку (скрыть из рекомендаций и переключить на следующий)",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
    ]
}

/// Выполнение вызова инструмента через Unix сокет IPC
async fn execute_tool_call(tool_name: &str, args: &serde_json::Value) -> serde_json::Value {
    let cmd = match tool_name {
        "volna_status" => serde_json::json!({ "cmd": "status" }),
        "volna_control" => {
            let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("toggle");
            serde_json::json!({ "cmd": action })
        }
        "volna_volume" => {
            let volume = args.get("volume").and_then(|v| v.as_f64()).unwrap_or(0.5);
            serde_json::json!({ "cmd": "volume", "val": volume })
        }
        "volna_switch_provider" => {
            let provider = args.get("provider").and_then(|v| v.as_str()).unwrap_or("yandex");
            serde_json::json!({ "cmd": "switch_provider", "provider": provider })
        }
        "volna_start_wave" => {
            let station_id = args.get("station_id").and_then(|v| v.as_str());
            serde_json::json!({ "cmd": "start_wave", "station_id": station_id })
        }
        "volna_wave_filter" => {
            let mut cmd = serde_json::Map::new();
            cmd.insert("cmd".into(), serde_json::json!("filter"));
            for key in ["language", "character", "mood"] {
                if let Some(value) = args.get(key).and_then(|v| v.as_str()) {
                    cmd.insert(key.into(), serde_json::json!(value));
                }
            }
            serde_json::Value::Object(cmd)
        }
        "volna_search_and_play" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            serde_json::json!({ "cmd": "search_and_play", "query": query })
        }
        "volna_set_cookie" => {
            let cookie = args.get("cookie").and_then(|v| v.as_str()).unwrap_or("");
            serde_json::json!({ "cmd": "set_cookie", "cookie": cookie })
        }
        "volna_like" => {
            if let Some(liked) = args.get("liked").and_then(|v| v.as_bool()) {
                serde_json::json!({ "cmd": "like", "liked": liked })
            } else {
                serde_json::json!({ "cmd": "like" })
            }
        }
        "volna_dislike" => serde_json::json!({ "cmd": "dislike" }),
        _ => {
            return serde_json::json!({
                "isError": true,
                "content": [
                    {
                        "type": "text",
                        "text": format!("Неизвестный инструмент: {}", tool_name)
                    }
                ]
            });
        }
    };

    match crate::ipc::send_cmd(&cmd).await {
        Ok(result) => {
            let is_err = result.get("ok").and_then(|v| v.as_bool()).map(|b| !b).unwrap_or(false);
            serde_json::json!({
                "isError": is_err,
                "content": [
                    {
                        "type": "text",
                        "text": serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string())
                    }
                ]
            })
        }
        Err(_) => {
            serde_json::json!({
                "isError": true,
                "content": [
                    {
                        "type": "text",
                        "text": "Плеер Волна не запущен (сокет volna.sock не найден)"
                    }
                ]
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mcp_initialize() {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        });
        let resp = handle_json_rpc(req).await.expect("initialize must respond");
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(resp["result"]["serverInfo"]["name"], "volna-mcp");
    }

    #[tokio::test]
    async fn test_mcp_notifications_no_response() {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let resp = handle_json_rpc(req).await;
        assert!(resp.is_none());
    }

    #[tokio::test]
    async fn test_mcp_ping() {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "ping"
        });
        let resp = handle_json_rpc(req).await.expect("ping must respond");
        assert_eq!(resp["id"], 42);
        assert!(resp["result"].as_object().is_some());
    }

    #[tokio::test]
    async fn test_mcp_tools_list() {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        });
        let resp = handle_json_rpc(req).await.expect("tools/list must respond");
        let tools = resp["result"]["tools"].as_array().expect("tools must be array");
        assert_eq!(tools.len(), 10);

        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"volna_status"));
        assert!(names.contains(&"volna_control"));
        assert!(names.contains(&"volna_volume"));
        assert!(names.contains(&"volna_switch_provider"));
        assert!(names.contains(&"volna_start_wave"));
        assert!(names.contains(&"volna_search_and_play"));
        assert!(names.contains(&"volna_set_cookie"));
        assert!(names.contains(&"volna_like"));
        assert!(names.contains(&"volna_dislike"));
    }

    #[tokio::test]
    async fn test_mcp_tools_call_socket_not_found() {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "volna_status",
                "arguments": {}
            }
        });
        // Если плеер не запущен, возвращается дружелюбная ошибка; если запущен, ответ плеера
        let resp = handle_json_rpc(req).await.expect("tools/call must respond");
        assert_eq!(resp["id"], 3);
        if resp["result"]["isError"] == true {
            let text = resp["result"]["content"][0]["text"].as_str().unwrap();
            assert!(text.contains("Плеер Волна не запущен (сокет volna.sock не найден)"));
        } else {
            assert_eq!(resp["result"]["isError"], false);
        }
    }
}
