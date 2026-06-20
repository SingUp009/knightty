//! Protocol extension parsing.

pub mod iterm2 {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;

    pub const MAX_METADATA_BYTES: usize = 4096;

    /// Cell-based dimension accepted by the F1 inline-image implementation.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum ImageDimension {
        Auto,
        Cells(u16),
    }

    /// Parsed iTerm2 `File=...:<payload>` inline-image command.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct InlineImageSequence<'a> {
        pub name: Option<String>,
        pub width: Option<ImageDimension>,
        pub height: Option<ImageDimension>,
        pub payload: &'a str,
    }

    /// Parse one iTerm2 inline-image OSC payload after the `1337;` prefix.
    ///
    /// ```
    /// use knightty_proto::iterm2::{ImageDimension, parse_iterm2_inline_image};
    ///
    /// let image = parse_iterm2_inline_image(
    ///     "File=inline=1;width=4;height=auto:AAAA",
    /// )
    /// .expect("valid inline image");
    ///
    /// assert_eq!(image.width, Some(ImageDimension::Cells(4)));
    /// assert_eq!(image.height, Some(ImageDimension::Auto));
    /// assert_eq!(image.payload, "AAAA");
    /// ```
    pub fn parse_iterm2_inline_image(
        sequence: &str,
    ) -> Result<InlineImageSequence<'_>, InlineImageParseError> {
        let command = sequence
            .strip_prefix("File=")
            .ok_or(InlineImageParseError::UnsupportedCommand)?;
        let (metadata, payload) = command
            .split_once(':')
            .ok_or(InlineImageParseError::MissingColon)?;
        if metadata.len() > MAX_METADATA_BYTES {
            return Err(InlineImageParseError::MetadataTooLarge);
        }
        if payload.is_empty() {
            return Err(InlineImageParseError::MissingPayload);
        }

        let mut inline = None;
        let mut name = None;
        let mut width = None;
        let mut height = None;
        let mut preserve_aspect_ratio = None;

        for attribute in metadata.split(';') {
            let (key, value) = attribute
                .split_once('=')
                .ok_or(InlineImageParseError::MalformedAttribute)?;
            if key.is_empty() || value.is_empty() {
                return Err(InlineImageParseError::MalformedAttribute);
            }

            match key {
                "inline" => {
                    set_once(&mut inline, value)?;
                }
                "name" => {
                    if name.is_some() {
                        return Err(InlineImageParseError::DuplicateAttribute);
                    }
                    let decoded = STANDARD
                        .decode(value)
                        .map_err(|_| InlineImageParseError::InvalidName)?;
                    name = Some(
                        String::from_utf8(decoded)
                            .map_err(|_| InlineImageParseError::InvalidName)?,
                    );
                }
                "width" => {
                    let dimension = parse_dimension(value)?;
                    set_once(&mut width, dimension)?;
                }
                "height" => {
                    let dimension = parse_dimension(value)?;
                    set_once(&mut height, dimension)?;
                }
                "preserveAspectRatio" => {
                    set_once(&mut preserve_aspect_ratio, value)?;
                }
                _ => {}
            }
        }

        match inline {
            Some("1") => {}
            Some(_) => return Err(InlineImageParseError::InlineDisabled),
            None => return Err(InlineImageParseError::MissingInline),
        }
        if preserve_aspect_ratio.is_some_and(|value| value != "1") {
            return Err(InlineImageParseError::UnsupportedAspectRatio);
        }

        Ok(InlineImageSequence {
            name,
            width,
            height,
            payload,
        })
    }

    fn set_once<T>(slot: &mut Option<T>, value: T) -> Result<(), InlineImageParseError> {
        if slot.replace(value).is_some() {
            Err(InlineImageParseError::DuplicateAttribute)
        } else {
            Ok(())
        }
    }

    fn parse_dimension(value: &str) -> Result<ImageDimension, InlineImageParseError> {
        if value == "auto" {
            return Ok(ImageDimension::Auto);
        }
        if !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(InlineImageParseError::UnsupportedDimension);
        }
        let cells = value
            .parse::<u16>()
            .map_err(|_| InlineImageParseError::UnsupportedDimension)?;
        if cells == 0 {
            return Err(InlineImageParseError::UnsupportedDimension);
        }
        Ok(ImageDimension::Cells(cells))
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum InlineImageParseError {
        UnsupportedCommand,
        MissingColon,
        MissingPayload,
        MetadataTooLarge,
        MalformedAttribute,
        DuplicateAttribute,
        MissingInline,
        InlineDisabled,
        InvalidName,
        UnsupportedDimension,
        UnsupportedAspectRatio,
    }

    #[cfg(test)]
    mod tests {
        use super::{
            ImageDimension, InlineImageParseError, MAX_METADATA_BYTES, parse_iterm2_inline_image,
        };
        use base64::Engine;
        use base64::engine::general_purpose::STANDARD;

        #[test]
        fn minimal_inline_image_parses() {
            let parsed =
                parse_iterm2_inline_image("File=inline=1:AAAA").expect("minimal image parses");

            assert_eq!(parsed.payload, "AAAA");
            assert_eq!(parsed.name, None);
            assert_eq!(parsed.width, None);
            assert_eq!(parsed.height, None);
        }

        #[test]
        fn name_is_base64_decoded_for_diagnostics() {
            let name = STANDARD.encode("sample.png");
            let sequence = format!("File=name={name};inline=1:AAAA");
            let parsed = parse_iterm2_inline_image(&sequence).expect("named image parses");

            assert_eq!(parsed.name.as_deref(), Some("sample.png"));
        }

        #[test]
        fn cell_dimensions_and_auto_parse() {
            let parsed = parse_iterm2_inline_image(
                "File=width=12;height=auto;preserveAspectRatio=1;inline=1:AAAA",
            )
            .expect("dimensions parse");

            assert_eq!(parsed.width, Some(ImageDimension::Cells(12)));
            assert_eq!(parsed.height, Some(ImageDimension::Auto));
        }

        #[test]
        fn unknown_attributes_are_ignored() {
            let parsed = parse_iterm2_inline_image("File=custom=value;inline=1:AAAA")
                .expect("unknown attribute is ignored");

            assert_eq!(parsed.payload, "AAAA");
        }

        #[test]
        fn duplicate_known_attribute_is_rejected() {
            let error = parse_iterm2_inline_image("File=inline=1;width=2;width=3:AAAA")
                .expect_err("duplicate width should fail");

            assert_eq!(error, InlineImageParseError::DuplicateAttribute);
        }

        #[test]
        fn malformed_attribute_is_rejected() {
            let error = parse_iterm2_inline_image("File=inline=1;broken:AAAA")
                .expect_err("malformed attribute should fail");

            assert_eq!(error, InlineImageParseError::MalformedAttribute);
        }

        #[test]
        fn missing_colon_is_rejected() {
            assert_eq!(
                parse_iterm2_inline_image("File=inline=1").unwrap_err(),
                InlineImageParseError::MissingColon
            );
        }

        #[test]
        fn missing_payload_is_rejected() {
            assert_eq!(
                parse_iterm2_inline_image("File=inline=1:").unwrap_err(),
                InlineImageParseError::MissingPayload
            );
        }

        #[test]
        fn inline_zero_is_rejected() {
            assert_eq!(
                parse_iterm2_inline_image("File=inline=0:AAAA").unwrap_err(),
                InlineImageParseError::InlineDisabled
            );
        }

        #[test]
        fn invalid_base64_name_is_rejected() {
            assert_eq!(
                parse_iterm2_inline_image("File=name=***;inline=1:AAAA").unwrap_err(),
                InlineImageParseError::InvalidName
            );
        }

        #[test]
        fn unsupported_dimensions_are_rejected() {
            for dimension in ["0", "25px", "10%", "-1"] {
                let sequence = format!("File=inline=1;width={dimension}:AAAA");
                assert_eq!(
                    parse_iterm2_inline_image(&sequence).unwrap_err(),
                    InlineImageParseError::UnsupportedDimension
                );
            }
        }

        #[test]
        fn preserve_aspect_ratio_zero_is_rejected() {
            assert_eq!(
                parse_iterm2_inline_image("File=inline=1;preserveAspectRatio=0:AAAA").unwrap_err(),
                InlineImageParseError::UnsupportedAspectRatio
            );
        }

        #[test]
        fn oversized_metadata_is_rejected() {
            let metadata = "x".repeat(MAX_METADATA_BYTES + 1);
            let sequence = format!("File={metadata}:AAAA");

            assert_eq!(
                parse_iterm2_inline_image(&sequence).unwrap_err(),
                InlineImageParseError::MetadataTooLarge
            );
        }
    }
}

pub mod kitty {
    const ESC: u8 = 0x1b;
    const MAX_CONTROL_DATA_BYTES: usize = 4096;
    pub const MAX_KITTY_CHUNK_BYTES: usize = 4096;

    #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub struct KittyImageKey {
        pub client_id: u32,
    }

    #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub struct KittyPlacementKey {
        pub client_image_id: u32,
        pub placement_id: u32,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum KittyAction {
        TransmitAndDisplay,
        Transmit,
        Place,
        Delete,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum KittyFormat {
        Png,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum KittyTransmission {
        Direct,
    }

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub enum KittyQuiet {
        #[default]
        Normal,
        SuppressSuccess,
        SuppressAll,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct KittyCommand<'a> {
        pub action: KittyAction,
        pub format: KittyFormat,
        pub transmission: KittyTransmission,
        pub image_id: Option<KittyImageKey>,
        pub placement_id: Option<u32>,
        pub columns: Option<u16>,
        pub rows: Option<u16>,
        pub cursor_movement: bool,
        pub quiet: KittyQuiet,
        pub more_chunks: bool,
        pub payload: &'a [u8],
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct KittyContinuation<'a> {
        pub more_chunks: bool,
        pub quiet: Option<KittyQuiet>,
        pub payload: &'a [u8],
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum ParsedKittyCommand<'a> {
        Command(KittyCommand<'a>),
        Continuation(KittyContinuation<'a>),
    }

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct KittyResponseContext {
        pub image_id: Option<u32>,
        pub placement_id: Option<u32>,
        pub quiet: KittyQuiet,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum KittyErrorCode {
        Invalid,
        NoEntry,
        NoData,
        BadPng,
        TooBig,
        NoSpace,
    }

    impl KittyErrorCode {
        pub const fn as_str(self) -> &'static str {
            match self {
                Self::Invalid => "EINVAL",
                Self::NoEntry => "ENOENT",
                Self::NoData => "ENODATA",
                Self::BadPng => "EBADPNG",
                Self::TooBig => "E2BIG",
                Self::NoSpace => "ENOSPC",
            }
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct KittyProtocolError {
        pub code: KittyErrorCode,
        pub message: &'static str,
    }

    impl KittyProtocolError {
        pub const fn new(code: KittyErrorCode, message: &'static str) -> Self {
            Self { code, message }
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum KittyParseError {
        MalformedControlData,
        DuplicateKey,
        InvalidValue,
        UnsupportedFeature,
        MissingImageId,
        MissingPayload,
        UnexpectedPayload,
    }

    impl KittyParseError {
        pub const fn protocol_error(self) -> KittyProtocolError {
            match self {
                Self::MissingPayload => {
                    KittyProtocolError::new(KittyErrorCode::NoData, "image payload is empty")
                }
                Self::MalformedControlData => {
                    KittyProtocolError::new(KittyErrorCode::Invalid, "malformed control data")
                }
                Self::DuplicateKey => {
                    KittyProtocolError::new(KittyErrorCode::Invalid, "duplicate control key")
                }
                Self::InvalidValue => {
                    KittyProtocolError::new(KittyErrorCode::Invalid, "invalid control value")
                }
                Self::UnsupportedFeature => {
                    KittyProtocolError::new(KittyErrorCode::Invalid, "unsupported graphics feature")
                }
                Self::MissingImageId => {
                    KittyProtocolError::new(KittyErrorCode::Invalid, "image id is required")
                }
                Self::UnexpectedPayload => KittyProtocolError::new(
                    KittyErrorCode::Invalid,
                    "payload is not allowed for this action",
                ),
            }
        }
    }

    /// Parse one Kitty graphics APC body after the leading `G` and before ST.
    ///
    /// ```
    /// use knightty_proto::kitty::{KittyAction, ParsedKittyCommand, parse_kitty_command};
    ///
    /// let ParsedKittyCommand::Command(command) =
    ///     parse_kitty_command(b"a=p,i=42,p=7,c=10,r=5").unwrap()
    /// else {
    ///     panic!("expected a complete command");
    /// };
    /// assert_eq!(command.action, KittyAction::Place);
    /// assert_eq!(command.image_id.unwrap().client_id, 42);
    /// assert_eq!(command.placement_id, Some(7));
    /// ```
    pub fn parse_kitty_command(input: &[u8]) -> Result<ParsedKittyCommand<'_>, KittyParseError> {
        let (control, payload, has_payload_separator) =
            match input.iter().position(|byte| *byte == b';') {
                Some(index) => (&input[..index], &input[index + 1..], true),
                None => (input, &[][..], false),
            };
        if control.len() > MAX_CONTROL_DATA_BYTES {
            return Err(KittyParseError::UnsupportedFeature);
        }

        let mut action = KittyAction::Transmit;
        let mut format = 32_u32;
        let mut transmission = b'd';
        let mut image_id = None;
        let mut placement_id = None;
        let mut columns = None;
        let mut rows = None;
        let mut cursor_movement = true;
        let mut quiet = KittyQuiet::Normal;
        let mut quiet_override = None;
        let mut more_chunks = false;
        let mut delete_selector = b'a';
        let mut seen = 0_u16;
        let mut continuation_control_only = true;

        if !control.is_empty() {
            for attribute in control.split(|byte| *byte == b',') {
                let Some(separator) = attribute.iter().position(|byte| *byte == b'=') else {
                    return Err(KittyParseError::MalformedControlData);
                };
                if separator == 0 || separator + 1 >= attribute.len() {
                    return Err(KittyParseError::MalformedControlData);
                }
                let key = &attribute[..separator];
                let value = &attribute[separator + 1..];
                if !matches!(key, b"m" | b"q") {
                    continuation_control_only = false;
                }
                let bit = match key {
                    b"a" => Some(1 << 0),
                    b"f" => Some(1 << 1),
                    b"t" => Some(1 << 2),
                    b"i" => Some(1 << 3),
                    b"p" => Some(1 << 4),
                    b"c" => Some(1 << 5),
                    b"r" => Some(1 << 6),
                    b"C" => Some(1 << 7),
                    b"q" => Some(1 << 8),
                    b"m" => Some(1 << 9),
                    b"d" => Some(1 << 10),
                    _ => None,
                };
                if let Some(bit) = bit {
                    if seen & bit != 0 {
                        return Err(KittyParseError::DuplicateKey);
                    }
                    seen |= bit;
                }

                match key {
                    b"a" => {
                        action = match single_byte(value)? {
                            b'T' => KittyAction::TransmitAndDisplay,
                            b't' => KittyAction::Transmit,
                            b'p' => KittyAction::Place,
                            b'd' => KittyAction::Delete,
                            _ => return Err(KittyParseError::UnsupportedFeature),
                        };
                    }
                    b"f" => format = parse_u32(value)?,
                    b"t" => transmission = single_byte(value)?,
                    b"i" => {
                        image_id = nonzero_u32(value)?.map(|client_id| KittyImageKey { client_id })
                    }
                    b"p" => placement_id = nonzero_u32(value)?,
                    b"c" => columns = optional_u16(value)?,
                    b"r" => rows = optional_u16(value)?,
                    b"C" => {
                        cursor_movement = match parse_u32(value)? {
                            0 => true,
                            1 => false,
                            _ => return Err(KittyParseError::InvalidValue),
                        };
                    }
                    b"q" => {
                        quiet = match parse_u32(value)? {
                            0 => KittyQuiet::Normal,
                            1 => KittyQuiet::SuppressSuccess,
                            2 => KittyQuiet::SuppressAll,
                            _ => return Err(KittyParseError::InvalidValue),
                        };
                        quiet_override = Some(quiet);
                    }
                    b"m" => {
                        more_chunks = match parse_u32(value)? {
                            0 => false,
                            1 => true,
                            _ => return Err(KittyParseError::InvalidValue),
                        };
                    }
                    b"d" => delete_selector = single_byte(value)?,
                    b"s" | b"v" | b"S" | b"O" | b"I" | b"o" | b"N" | b"x" | b"y" | b"w" | b"h"
                    | b"X" | b"Y" | b"U" | b"z" | b"P" | b"Q" | b"H" | b"V" => {
                        return Err(KittyParseError::UnsupportedFeature);
                    }
                    _ => {}
                }
            }
        }

        let has_more_key = seen & (1 << 9) != 0;
        if has_more_key && continuation_control_only {
            if !has_payload_separator {
                return Err(KittyParseError::MalformedControlData);
            }
            return Ok(ParsedKittyCommand::Continuation(KittyContinuation {
                more_chunks,
                quiet: quiet_override,
                payload,
            }));
        }

        if transmission != b'd' {
            return Err(KittyParseError::UnsupportedFeature);
        }
        match action {
            KittyAction::Transmit | KittyAction::TransmitAndDisplay => {
                if format != 100 {
                    return Err(KittyParseError::UnsupportedFeature);
                }
                if more_chunks && image_id.is_none() {
                    return Err(KittyParseError::MissingImageId);
                }
                if !has_payload_separator || (!more_chunks && payload.is_empty()) {
                    return Err(KittyParseError::MissingPayload);
                }
            }
            KittyAction::Place => {
                if more_chunks {
                    return Err(KittyParseError::UnsupportedFeature);
                }
                if image_id.is_none() {
                    return Err(KittyParseError::MissingImageId);
                }
                if !payload.is_empty() {
                    return Err(KittyParseError::UnexpectedPayload);
                }
            }
            KittyAction::Delete => {
                if more_chunks {
                    return Err(KittyParseError::UnsupportedFeature);
                }
                if delete_selector != b'i' {
                    return Err(KittyParseError::UnsupportedFeature);
                }
                if image_id.is_none() {
                    return Err(KittyParseError::MissingImageId);
                }
                if !payload.is_empty() {
                    return Err(KittyParseError::UnexpectedPayload);
                }
            }
        }

        Ok(ParsedKittyCommand::Command(KittyCommand {
            action,
            format: KittyFormat::Png,
            transmission: KittyTransmission::Direct,
            image_id,
            placement_id,
            columns,
            rows,
            cursor_movement,
            quiet,
            more_chunks,
            payload,
        }))
    }

    pub fn response_context(input: &[u8]) -> KittyResponseContext {
        let control = input.split(|byte| *byte == b';').next().unwrap_or(input);
        let mut context = KittyResponseContext::default();
        for attribute in control.split(|byte| *byte == b',') {
            let Some(separator) = attribute.iter().position(|byte| *byte == b'=') else {
                continue;
            };
            let key = &attribute[..separator];
            let value = &attribute[separator + 1..];
            match key {
                b"i" => context.image_id = nonzero_u32(value).ok().flatten(),
                b"p" => context.placement_id = nonzero_u32(value).ok().flatten(),
                b"q" => {
                    context.quiet = match parse_u32(value) {
                        Ok(1) => KittyQuiet::SuppressSuccess,
                        Ok(2) => KittyQuiet::SuppressAll,
                        _ => KittyQuiet::Normal,
                    };
                }
                _ => {}
            }
        }
        context
    }

    pub fn encode_response(
        context: KittyResponseContext,
        result: Result<(), KittyProtocolError>,
    ) -> Option<Vec<u8>> {
        let image_id = context.image_id?;
        if (result.is_ok() && context.quiet != KittyQuiet::Normal)
            || (result.is_err() && context.quiet == KittyQuiet::SuppressAll)
        {
            return None;
        }

        let mut response = format!("\x1b_Gi={image_id}");
        if let Some(placement_id) = context.placement_id {
            response.push_str(&format!(",p={placement_id}"));
        }
        response.push(';');
        match result {
            Ok(()) => response.push_str("OK"),
            Err(error) => {
                response.push_str(error.code.as_str());
                if !error.message.is_empty() {
                    response.push(':');
                    response.push_str(error.message);
                }
            }
        }
        response.push_str("\x1b\\");
        Some(response.into_bytes())
    }

    fn single_byte(value: &[u8]) -> Result<u8, KittyParseError> {
        match value {
            [byte] => Ok(*byte),
            _ => Err(KittyParseError::InvalidValue),
        }
    }

    fn parse_u32(value: &[u8]) -> Result<u32, KittyParseError> {
        if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
            return Err(KittyParseError::InvalidValue);
        }
        let mut parsed = 0_u32;
        for byte in value {
            parsed = parsed
                .checked_mul(10)
                .and_then(|number| number.checked_add(u32::from(*byte - b'0')))
                .ok_or(KittyParseError::InvalidValue)?;
        }
        Ok(parsed)
    }

    fn nonzero_u32(value: &[u8]) -> Result<Option<u32>, KittyParseError> {
        Ok(match parse_u32(value)? {
            0 => None,
            value => Some(value),
        })
    }

    fn optional_u16(value: &[u8]) -> Result<Option<u16>, KittyParseError> {
        let value = parse_u32(value)?;
        if value == 0 {
            Ok(None)
        } else {
            u16::try_from(value)
                .map(Some)
                .map_err(|_| KittyParseError::InvalidValue)
        }
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum GraphicsStreamItem {
        Vte(Vec<u8>),
        Kitty(Vec<u8>),
        OversizedKitty { control_data: Vec<u8> },
    }

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    enum StreamState {
        #[default]
        Ground,
        Escape,
        ApcKind,
        NonKittyApc,
        NonKittyEscape,
        Kitty,
        KittyEscape,
        OversizedKitty,
        OversizedKittyEscape,
    }

    #[derive(Debug)]
    pub struct GraphicsEscapeRouter {
        state: StreamState,
        kitty_buffer: Vec<u8>,
        kitty_control_end: Option<usize>,
        max_payload_bytes: usize,
    }

    impl GraphicsEscapeRouter {
        pub fn new(max_payload_bytes: usize) -> Self {
            Self {
                state: StreamState::Ground,
                kitty_buffer: Vec::with_capacity(4096),
                kitty_control_end: None,
                max_payload_bytes,
            }
        }

        pub fn set_max_payload_bytes(&mut self, max_payload_bytes: usize) {
            self.max_payload_bytes = max_payload_bytes;
        }

        pub fn feed(&mut self, input: &[u8]) -> Vec<GraphicsStreamItem> {
            let mut items = Vec::new();
            let mut vte = Vec::with_capacity(input.len());
            for byte in input.iter().copied() {
                match self.state {
                    StreamState::Ground => {
                        if byte == ESC {
                            self.state = StreamState::Escape;
                        } else {
                            vte.push(byte);
                        }
                    }
                    StreamState::Escape => {
                        if byte == b'_' {
                            self.state = StreamState::ApcKind;
                        } else {
                            vte.push(ESC);
                            if byte == ESC {
                                self.state = StreamState::Escape;
                            } else {
                                vte.push(byte);
                                self.state = StreamState::Ground;
                            }
                        }
                    }
                    StreamState::ApcKind => {
                        if byte == b'G' {
                            flush_vte(&mut items, &mut vte);
                            self.kitty_buffer.clear();
                            self.kitty_control_end = None;
                            self.state = StreamState::Kitty;
                        } else {
                            vte.extend_from_slice(b"\x1b_");
                            vte.push(byte);
                            self.state = StreamState::NonKittyApc;
                        }
                    }
                    StreamState::NonKittyApc => {
                        vte.push(byte);
                        if byte == ESC {
                            self.state = StreamState::NonKittyEscape;
                        }
                    }
                    StreamState::NonKittyEscape => {
                        vte.push(byte);
                        self.state = if byte == b'\\' {
                            StreamState::Ground
                        } else if byte == ESC {
                            StreamState::NonKittyEscape
                        } else {
                            StreamState::NonKittyApc
                        };
                    }
                    StreamState::Kitty => {
                        if byte == ESC {
                            self.state = StreamState::KittyEscape;
                        } else {
                            self.push_kitty_byte(byte);
                        }
                    }
                    StreamState::KittyEscape => {
                        if byte == b'\\' {
                            items.push(GraphicsStreamItem::Kitty(core::mem::take(
                                &mut self.kitty_buffer,
                            )));
                            self.kitty_control_end = None;
                            self.state = StreamState::Ground;
                        } else {
                            self.push_kitty_byte(ESC);
                            if self.state != StreamState::OversizedKitty {
                                if byte == ESC {
                                    self.state = StreamState::KittyEscape;
                                } else {
                                    self.push_kitty_byte(byte);
                                }
                            }
                        }
                    }
                    StreamState::OversizedKitty => {
                        if byte == ESC {
                            self.state = StreamState::OversizedKittyEscape;
                        }
                    }
                    StreamState::OversizedKittyEscape => {
                        if byte == b'\\' {
                            let end = self
                                .kitty_control_end
                                .unwrap_or(self.kitty_buffer.len())
                                .min(self.kitty_buffer.len());
                            self.kitty_buffer.truncate(end);
                            items.push(GraphicsStreamItem::OversizedKitty {
                                control_data: core::mem::take(&mut self.kitty_buffer),
                            });
                            self.kitty_control_end = None;
                            self.state = StreamState::Ground;
                        } else if byte != ESC {
                            self.state = StreamState::OversizedKitty;
                        }
                    }
                }
            }
            flush_vte(&mut items, &mut vte);
            items
        }

        fn push_kitty_byte(&mut self, byte: u8) {
            if self.state == StreamState::OversizedKitty {
                return;
            }
            if byte == b';' && self.kitty_control_end.is_none() {
                self.kitty_control_end = Some(self.kitty_buffer.len());
            }
            self.kitty_buffer.push(byte);
            let oversized = match self.kitty_control_end {
                Some(end) => {
                    self.kitty_buffer.len().saturating_sub(end + 1) > self.max_payload_bytes
                }
                None => self.kitty_buffer.len() > MAX_CONTROL_DATA_BYTES,
            };
            if oversized {
                self.kitty_buffer.truncate(
                    self.kitty_control_end
                        .unwrap_or(MAX_CONTROL_DATA_BYTES)
                        .min(self.kitty_buffer.len()),
                );
                self.state = StreamState::OversizedKitty;
            } else if self.state != StreamState::KittyEscape {
                self.state = StreamState::Kitty;
            }
        }
    }

    impl Default for GraphicsEscapeRouter {
        fn default() -> Self {
            Self::new(16 * 1024 * 1024)
        }
    }

    fn flush_vte(items: &mut Vec<GraphicsStreamItem>, vte: &mut Vec<u8>) {
        if !vte.is_empty() {
            items.push(GraphicsStreamItem::Vte(core::mem::take(vte)));
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{
            GraphicsEscapeRouter, GraphicsStreamItem, KittyAction, KittyErrorCode, KittyParseError,
            KittyProtocolError, KittyQuiet, KittyResponseContext, ParsedKittyCommand,
            encode_response, parse_kitty_command, response_context,
        };

        fn command(input: &[u8]) -> super::KittyCommand<'_> {
            match parse_kitty_command(input).expect("Kitty command parses") {
                ParsedKittyCommand::Command(command) => command,
                ParsedKittyCommand::Continuation(_) => panic!("expected a complete command"),
            }
        }

        #[test]
        fn fragmented_kitty_apc_is_captured_without_exposing_payload_to_vte() {
            let mut router = GraphicsEscapeRouter::new(1024);

            assert_eq!(
                router.feed(b"before\x1b"),
                vec![GraphicsStreamItem::Vte(b"before".to_vec())]
            );
            assert!(router.feed(b"_Gf=100,i=42;AAAA\x1b").is_empty());
            assert_eq!(
                router.feed(b"\\after"),
                vec![
                    GraphicsStreamItem::Kitty(b"f=100,i=42;AAAA".to_vec()),
                    GraphicsStreamItem::Vte(b"after".to_vec()),
                ]
            );
        }

        #[test]
        fn kitty_control_payload_and_st_can_each_cross_read_boundaries() {
            let mut router = GraphicsEscapeRouter::new(1024);

            assert!(router.feed(b"\x1b_Ga=T,f=100,i=42").is_empty());
            assert!(router.feed(b",m=1;").is_empty());
            assert!(router.feed(b"AAAA\x1b").is_empty());
            assert_eq!(
                router.feed(b"\\"),
                vec![GraphicsStreamItem::Kitty(
                    b"a=T,f=100,i=42,m=1;AAAA".to_vec()
                )]
            );
        }

        #[test]
        fn multiple_kitty_apcs_and_utf8_are_routed_in_order_from_one_read() {
            let mut router = GraphicsEscapeRouter::new(1024);
            let input =
                b"\xe5\x89\x8d\x1b_Ga=t,f=100,i=1,m=1;AAAA\x1b\\\x1b_Gm=0;\x1b\\\xe5\xbe\x8c";

            assert_eq!(
                router.feed(input),
                vec![
                    GraphicsStreamItem::Vte("前".as_bytes().to_vec()),
                    GraphicsStreamItem::Kitty(b"a=t,f=100,i=1,m=1;AAAA".to_vec()),
                    GraphicsStreamItem::Kitty(b"m=0;".to_vec()),
                    GraphicsStreamItem::Vte("後".as_bytes().to_vec()),
                ]
            );
        }

        #[test]
        fn ordinary_sequences_and_non_kitty_apc_pass_through_exactly() {
            let mut router = GraphicsEscapeRouter::new(1024);
            let input = b"A\x1b[31mB\x1b_Xpayload\x1b\\C\x1b]0;title\x07";

            assert_eq!(
                router.feed(input),
                vec![GraphicsStreamItem::Vte(input.to_vec())]
            );
        }

        #[test]
        fn oversized_kitty_payload_is_discarded_and_router_recovers() {
            let mut router = GraphicsEscapeRouter::new(4);

            assert_eq!(
                router.feed(b"\x1b_Gf=100,i=9;AAAAA\x1b\\ok"),
                vec![
                    GraphicsStreamItem::OversizedKitty {
                        control_data: b"f=100,i=9".to_vec(),
                    },
                    GraphicsStreamItem::Vte(b"ok".to_vec()),
                ]
            );
        }

        #[test]
        fn transmit_place_and_delete_commands_parse() {
            let transmit = command(b"a=T,f=100,t=d,i=42,p=7,c=10,r=5,C=1,q=1,m=0;AAAA");
            assert_eq!(transmit.action, KittyAction::TransmitAndDisplay);
            assert_eq!(transmit.image_id.unwrap().client_id, 42);
            assert_eq!(transmit.placement_id, Some(7));
            assert_eq!(transmit.columns, Some(10));
            assert_eq!(transmit.rows, Some(5));
            assert!(!transmit.cursor_movement);
            assert_eq!(transmit.quiet, KittyQuiet::SuppressSuccess);
            assert!(!transmit.more_chunks);

            assert_eq!(command(b"a=p,i=42").action, KittyAction::Place);
            assert_eq!(command(b"a=d,d=i,i=42").action, KittyAction::Delete);
        }

        #[test]
        fn multipart_initial_and_continuation_chunks_parse() {
            let initial = command(b"a=T,f=100,t=d,i=42,p=7,q=1,m=1;AAAA");
            assert_eq!(initial.action, KittyAction::TransmitAndDisplay);
            assert_eq!(initial.image_id.unwrap().client_id, 42);
            assert_eq!(initial.placement_id, Some(7));
            assert_eq!(initial.quiet, KittyQuiet::SuppressSuccess);
            assert!(initial.more_chunks);
            assert_eq!(initial.payload, b"AAAA");

            assert_eq!(
                parse_kitty_command(b"m=1,q=2;").unwrap(),
                ParsedKittyCommand::Continuation(super::KittyContinuation {
                    more_chunks: true,
                    quiet: Some(KittyQuiet::SuppressAll),
                    payload: b"",
                })
            );
            assert_eq!(
                parse_kitty_command(b"m=0;").unwrap(),
                ParsedKittyCommand::Continuation(super::KittyContinuation {
                    more_chunks: false,
                    quiet: None,
                    payload: b"",
                })
            );
        }

        #[test]
        fn parser_rejects_invalid_known_values_and_unsupported_features() {
            assert_eq!(
                parse_kitty_command(b"a=T,f=24,i=1;AAAA").unwrap_err(),
                KittyParseError::UnsupportedFeature
            );
            assert_eq!(
                parse_kitty_command(b"a=T,f=100,m=1;AAAA").unwrap_err(),
                KittyParseError::MissingImageId
            );
            assert_eq!(
                parse_kitty_command(b"a=p,i=1,C=2").unwrap_err(),
                KittyParseError::InvalidValue
            );
            assert_eq!(
                parse_kitty_command(b"a=p,i=1,i=2").unwrap_err(),
                KittyParseError::DuplicateKey
            );
            assert_eq!(
                parse_kitty_command(b"a=p,i=1,z=2").unwrap_err(),
                KittyParseError::UnsupportedFeature
            );
            assert_eq!(
                parse_kitty_command(b"a=p,i=1,m=1;").unwrap_err(),
                KittyParseError::UnsupportedFeature
            );
            assert_eq!(
                parse_kitty_command(b"m=1").unwrap_err(),
                KittyParseError::MalformedControlData
            );
            assert_eq!(
                parse_kitty_command(b"m=1,custom=value;AAAA").unwrap_err(),
                KittyParseError::UnsupportedFeature
            );
        }

        #[test]
        fn unknown_keys_are_ignored_and_zero_ids_are_unspecified() {
            let command = command(b"a=T,f=100,i=0,p=0,custom=value;AAAA");
            assert_eq!(command.image_id, None);
            assert_eq!(command.placement_id, None);
        }

        #[test]
        fn responses_include_ids_and_honor_quiet_policy() {
            let context = KittyResponseContext {
                image_id: Some(42),
                placement_id: Some(7),
                quiet: KittyQuiet::Normal,
            };
            assert_eq!(
                encode_response(context, Ok(())).unwrap(),
                b"\x1b_Gi=42,p=7;OK\x1b\\"
            );
            assert_eq!(
                encode_response(
                    context,
                    Err(KittyProtocolError::new(
                        KittyErrorCode::NoEntry,
                        "image not found"
                    ))
                )
                .unwrap(),
                b"\x1b_Gi=42,p=7;ENOENT:image not found\x1b\\"
            );
            assert!(
                encode_response(
                    KittyResponseContext {
                        quiet: KittyQuiet::SuppressSuccess,
                        ..context
                    },
                    Ok(())
                )
                .is_none()
            );
            assert!(
                encode_response(
                    KittyResponseContext {
                        quiet: KittyQuiet::SuppressAll,
                        ..context
                    },
                    Err(KittyProtocolError::new(
                        KittyErrorCode::Invalid,
                        "bad command"
                    ))
                )
                .is_none()
            );
        }

        #[test]
        fn response_context_is_recovered_from_invalid_control_data() {
            assert_eq!(
                response_context(b"a=T,i=42,p=3,q=2,C=9"),
                KittyResponseContext {
                    image_id: Some(42),
                    placement_id: Some(3),
                    quiet: KittyQuiet::SuppressAll,
                }
            );
        }
    }
}
