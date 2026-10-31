//! Типизированная ошибка клиента YouTube Music.

/// Ошибка запроса к YouTube Music.
///
/// Заменяет `Box<dyn Error + Send + Sync>` во внутренних функциях клиента, чтобы
/// вызывающий код мог отличить отказ авторизации от сетевого сбоя: см.
/// [`YtMusicError::is_auth`] и [`YtMusicError::is_retryable`]. Текст ошибки
/// (`Display`) совпадает с прежним, поэтому сообщения в интерфейсе не меняются.
#[derive(Debug, thiserror::Error)]
pub enum YtMusicError {
    /// Сетевая ошибка: DNS, TLS, таймаут, обрыв соединения.
    #[error(transparent)]
    Network(#[from] reqwest::Error),

    /// Ответ получен, но YouTube вернул не 2xx.
    ///
    /// `status` хранится отдельно, чтобы отличить 401/403 (нужна авторизация)
    /// от 429/5xx (запрос имеет смысл повторить).
    #[error("HTTP {status}: {body}")]
    Http {
        status: reqwest::StatusCode,
        body: String,
    },

    /// Ошибка локального ввода-вывода: кэш, файлы, запуск yt-dlp.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Ответ пришёл, но пригодных данных в нём нет, либо локальная проверка не прошла.
    #[error("{0}")]
    Message(String),
}

impl YtMusicError {
    /// YouTube отклонил запрос из-за отсутствия или просрочки авторизации.
    pub fn is_auth(&self) -> bool {
        matches!(
            self,
            Self::Http { status, .. }
                if *status == reqwest::StatusCode::UNAUTHORIZED
                    || *status == reqwest::StatusCode::FORBIDDEN
        )
    }

    /// Сбой временный: тот же запрос имеет смысл повторить (сеть, 429 или 5xx).
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Network(_) => true,
            Self::Http { status, .. } => {
                status.is_server_error() || *status == reqwest::StatusCode::TOO_MANY_REQUESTS
            }
            Self::Io(_) | Self::Message(_) => false,
        }
    }
}

impl From<String> for YtMusicError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for YtMusicError {
    fn from(message: &str) -> Self {
        Self::Message(message.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_display_is_verbatim() {
        let err: YtMusicError = "Исполнитель не найден".to_string().into();
        assert_eq!(err.to_string(), "Исполнитель не найден");
        assert!(!err.is_auth() && !err.is_retryable());
    }

    #[test]
    fn http_display_matches_old_format() {
        let err = YtMusicError::Http {
            status: reqwest::StatusCode::UNAUTHORIZED,
            body: "no token".to_string(),
        };
        assert_eq!(err.to_string(), "HTTP 401 Unauthorized: no token");
        assert!(err.is_auth() && !err.is_retryable());
    }

    #[test]
    fn server_errors_are_retryable_but_not_auth() {
        let err = YtMusicError::Http {
            status: reqwest::StatusCode::BAD_GATEWAY,
            body: String::new(),
        };
        assert!(err.is_retryable() && !err.is_auth());
    }
}
