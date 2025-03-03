use std::fmt;

/// TwiML response builder for Twilio voice responses
pub struct TwiML {
    content: String,
}

impl TwiML {
    /// Create a new TwiML response
    pub fn new() -> Self {
        TwiML {
            content: String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Response>"),
        }
    }
    
    /// Add a Say verb to the response
    pub fn say(mut self, text: &str, voice: &str, language: Option<&str>) -> Self {
        self.content.push_str("<Say");
        
        if !voice.is_empty() {
            self.content.push_str(&format!(" voice=\"{}\"", escape_xml_attr(voice)));
        }
        
        if let Some(lang) = language {
            if !lang.is_empty() {
                self.content.push_str(&format!(" language=\"{}\"", escape_xml_attr(lang)));
            }
        }
        
        self.content.push_str(&format!(">{}</Say>", escape_xml(text)));
        self
    }
    
    /// Add a Gather verb to the response
    pub fn gather(mut self, options: GatherOptions) -> Self {
        self.content.push_str("<Gather");
        
        if let Some(input) = options.input {
            self.content.push_str(&format!(" input=\"{}\"", escape_xml_attr(input)));
        }
        
        if let Some(action) = options.action {
            self.content.push_str(&format!(" action=\"{}\"", escape_xml_attr(action)));
        }
        
        if let Some(method) = options.method {
            self.content.push_str(&format!(" method=\"{}\"", escape_xml_attr(method)));
        }
        
        if let Some(timeout) = options.timeout {
            self.content.push_str(&format!(" timeout=\"{}\"", timeout));
        }
        
        if let Some(speech_timeout) = options.speech_timeout {
            self.content.push_str(&format!(" speechTimeout=\"{}\"", escape_xml_attr(speech_timeout)));
        }
        
        if let Some(barge_in) = options.barge_in {
            self.content.push_str(&format!(" bargeIn=\"{}\"", barge_in));
        }
        
        if let Some(partial_result_callback) = options.partial_result_callback {
            self.content.push_str(&format!(" partialResultCallback=\"{}\"", escape_xml_attr(partial_result_callback)));
        }
        
        if let Some(speech_model) = options.speech_model {
            self.content.push_str(&format!(" speechModel=\"{}\"", escape_xml_attr(speech_model)));
        }
        
        if let Some(language) = options.language {
            self.content.push_str(&format!(" language=\"{}\"", escape_xml_attr(language)));
        }
        
        self.content.push_str(">");
        
        if let Some(say_text) = options.say_text {
            self.content.push_str(&format!(
                "<Say{}{}>{}</Say>",
                if let Some(voice) = options.voice {
                    format!(" voice=\"{}\"", escape_xml_attr(voice))
                } else {
                    String::new()
                },
                if let Some(language) = options.language {
                    format!(" language=\"{}\"", escape_xml_attr(language))
                } else {
                    String::new()
                },
                escape_xml(&say_text)
            ));
        }
        
        self.content.push_str("</Gather>");
        self
    }
    
    /// Add a Hangup verb to the response
    pub fn hangup(mut self) -> Self {
        self.content.push_str("<Hangup/>");
        self
    }
    
    /// Add a Redirect verb to the response
    pub fn redirect(mut self, url: &str) -> Self {
        self.content.push_str(&format!("<Redirect>{}</Redirect>", escape_xml(url)));
        self
    }
    
    /// Add a Play verb to the response with digits
    pub fn play_digits(mut self, digits: &str) -> Self {
        self.content.push_str(&format!("<Play digits=\"{}\"/>", escape_xml_attr(digits)));
        self
    }
    
    /// Add a Pause verb to the response
    pub fn pause(mut self, length: u32) -> Self {
        self.content.push_str(&format!("<Pause length=\"{}\"/>", length));
        self
    }
    
    /// Finalize the TwiML response
    pub fn build(mut self) -> String {
        self.content.push_str("</Response>");
        self.content
    }
}

impl fmt::Display for TwiML {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let response = format!("{}</Response>", self.content);
        write!(f, "{}", response)
    }
}

/// Options for the Gather TwiML verb
pub struct GatherOptions<'a> {
    pub input: Option<&'a str>,
    pub action: Option<&'a str>,
    pub method: Option<&'a str>,
    pub timeout: Option<u32>,
    pub speech_timeout: Option<&'a str>,
    pub barge_in: Option<bool>,
    pub partial_result_callback: Option<&'a str>,
    pub speech_model: Option<&'a str>,
    pub language: Option<&'a str>,
    pub say_text: Option<&'a str>,
    pub voice: Option<&'a str>,
}

impl<'a> Default for GatherOptions<'a> {
    fn default() -> Self {
        GatherOptions {
            input: Some("speech"),
            action: None,
            method: Some("POST"),
            timeout: Some(10),
            speech_timeout: Some("auto"),
            barge_in: Some(true),
            partial_result_callback: None,
            speech_model: None,
            language: None,
            say_text: None,
            voice: None,
        }
    }
}

/// Helper function to create a voice response with a Gather verb
pub fn create_voice_response(
    text: &str,
    config: &crate::config::TwilioConfig,
    timeout: u32,
    speech_timeout: &str
) -> String {
    // Create longer-lived strings first
    let action_url = format!("{}{}", config.webhook_url, "/transcription_callback");
    let partial_callback_url = format!("{}{}", config.webhook_url, "/partial_callback");

    let gather_options = GatherOptions {
        input: Some("speech"),
        action: Some(&action_url),
        method: Some("POST"),
        timeout: Some(timeout),
        speech_timeout: Some(speech_timeout),
        barge_in: Some(true),
        partial_result_callback: Some(&partial_callback_url),
        speech_model: Some(&config.speech_model),
        language: config.language.as_deref(),
        say_text: Some(text),
        voice: Some(&config.voice),
    };

    TwiML::new()
        .gather(gather_options)
        .build()
}

/// Helper function to create a hangup response
pub fn create_hangup_response(text: Option<&str>, config: &crate::config::TwilioConfig) -> String {
    let mut twiml = TwiML::new();
    
    if let Some(message) = text {
        twiml = twiml.say(message, &config.voice, config.language.as_deref());
    }
    
    twiml.hangup().build()
}

/// Escape XML text content
fn escape_xml(s: &str) -> String {
    s.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
}

/// Escape XML attribute values
fn escape_xml_attr(s: &str) -> String {
    escape_xml(s)
        .replace("\"", "&quot;")
        .replace("'", "&apos;")
}

/// Helper function to determine if text ends with sentence punctuation
pub fn ends_with_sentence_punctuation(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.ends_with(".") || trimmed.ends_with("!") || trimmed.ends_with("?")
}