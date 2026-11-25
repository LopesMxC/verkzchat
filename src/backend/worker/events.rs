//! Eventos do WhatsApp: o cartão do evento, o convite que o compartilha em
//! outra conversa, a capa que chega numa mensagem à parte e as edições, que
//! chegam cifradas com o segredo do evento, como os votos de enquete.

use super::*;

/// O conteúdo de um evento ou de um convite de evento, ou `None` quando a
/// mensagem não é nenhum dos dois. A capa entra quando a mensagem já a traz
/// junto (uma capa que chegou antes e foi guardada com o evento).
pub(super) fn content(base: &wa::Message) -> Option<Content> {
    if let Some(event) = base.event_message.as_option() {
        let location = event.location.as_option();
        let place = location
            .and_then(|location| non_empty(&location.name))
            .or_else(|| location.and_then(|location| non_empty(&location.address)));
        return Some(Content::Event {
            name: non_empty(&event.name).unwrap_or_else(|| "Event".to_owned()),
            description: non_empty(&event.description),
            start: seconds(event.start_time),
            end: seconds(event.end_time),
            place,
            call_link: non_empty(&event.join_link),
            canceled: event.is_canceled.unwrap_or(false),
            invite: false,
            cover: cover_media(base),
        });
    }
    if let Some(invite) = base.event_invite_message.as_option() {
        use wa::message::event_invite_message::EventStatus;
        let canceled = invite.is_canceled.unwrap_or(false)
            || matches!(
                invite.status,
                Some(EventStatus::CANCELED | EventStatus::DELETED)
            );
        return Some(Content::Event {
            name: non_empty(&invite.event_title).unwrap_or_else(|| "Event".to_owned()),
            description: non_empty(&invite.caption),
            start: seconds(invite.start_time),
            end: seconds(invite.end_time),
            place: non_empty(&invite.location_name),
            call_link: non_empty(&invite.call_link),
            canceled,
            invite: true,
            cover: None,
        });
    }
    None
}

/// A capa já guardada junto do evento, como mídia para baixar.
fn cover_media(base: &wa::Message) -> Option<Media> {
    let image = base.image_message.as_option()?;
    Some(media(
        image.mimetype.as_ref(),
        image.file_length,
        image.width,
        image.height,
    ))
}

/// Um horário só vale quando é positivo. O WhatsApp manda segundos, mas um
/// valor em milissegundos é reconhecido pelo tamanho.
fn seconds(value: Option<i64>) -> Option<i64> {
    let value = value.filter(|value| *value > 0)?;
    Some(if value > 100_000_000_000 {
        value / 1000
    } else {
        value
    })
}

/// Se `base` é a capa de um evento: uma foto presa ao evento, que não é uma
/// mensagem própria na conversa.
pub(super) fn is_cover(base: &wa::Message) -> bool {
    cover_of(base).is_some()
}

/// A capa de um evento: o id do evento e a foto.
pub(super) fn cover_of(base: &wa::Message) -> Option<(String, wa::message::ImageMessage)> {
    use wa::message_association::AssociationType;
    let child = base
        .associated_child_message
        .as_option()
        .and_then(|wrapper| wrapper.message.as_option());
    let association = |message: &wa::Message| {
        message
            .message_context_info
            .as_option()
            .and_then(|context| context.message_association.as_option())
            .filter(|association| {
                association.association_type == Some(AssociationType::EVENT_COVER_IMAGE)
            })
            .and_then(|association| association.parent_message_key.as_option())
            .and_then(|key| key.id.clone())
            .filter(|id| !id.is_empty())
    };
    let parent = association(base).or_else(|| child.and_then(association))?;
    let image = child
        .and_then(|child| child.image_message.as_option())
        .or(base.image_message.as_option())?
        .clone();
    Some((parent, image))
}

/// O evento guardado em `raw` com a capa ao lado, para que o download da capa
/// ache as chaves dela no próprio evento. Os envelopes ficam de fora, mas o
/// segredo do evento é mantido: as edições e as respostas precisam dele.
pub(super) fn with_cover(raw: &[u8], image: &wa::message::ImageMessage) -> Option<wa::Message> {
    let message = wa::Message::decode_from_slice(raw).ok()?;
    let mut merged = message.get_base_message().clone();
    if !merged.event_message.is_set() {
        return None;
    }
    if merged.message_context_info.is_unset() {
        merged.message_context_info = message.message_context_info.clone();
    }
    merged.image_message = MessageField::some(image.clone());
    Some(merged)
}

impl Worker {
    /// Guarda a capa de um evento com ele. Uma capa que chega antes do evento
    /// espera por ele.
    ///
    /// `sender`, quando se sabe quem mandou a capa, tem de ser quem criou o
    /// evento: ninguém mais põe capa no evento de outra pessoa.
    pub(super) fn file_event_cover(
        &mut self,
        chat: &str,
        parent: &str,
        sender: Option<&str>,
        image: wa::message::ImageMessage,
    ) {
        let Ok(Some(raw)) = self.archive.raw(chat, parent) else {
            if self.waiting_covers.len() < 256 {
                self.waiting_covers.insert(
                    (chat.to_owned(), parent.to_owned()),
                    (sender.map(str::to_owned), image),
                );
            }
            return;
        };
        let Some(merged) = with_cover(&raw, &image) else {
            return;
        };
        let Ok(Some(mut row)) = self.archive.message(chat, parent) else {
            return;
        };
        if let Some(sender) = sender
            && sender != row.sender
        {
            log::debug!("ignored an event cover from someone other than its creator");
            return;
        }
        let Content::Event { cover, .. } = &mut row.content else {
            return;
        };
        *cover = cover_media(&merged);
        let thumbnail = image
            .jpeg_thumbnail
            .clone()
            .filter(|bytes| !bytes.is_empty());
        match self.archive.set_event_cover(
            chat,
            parent,
            &merged.encode_to_vec(),
            &row.content,
            row.edited,
            thumbnail.as_deref(),
        ) {
            Ok(true) => self.emit_message(chat, parent),
            Ok(false) => {}
            Err(error) => log::warn!("could not store an event cover: {error}"),
        }
    }

    /// Aplica a capa que chegou antes do evento `id`.
    pub(super) fn settle_event_cover(&mut self, chat: &str, id: &str) {
        if let Some((sender, image)) = self
            .waiting_covers
            .remove(&(chat.to_owned(), id.to_owned()))
        {
            self.file_event_cover(chat, id, sender.as_deref(), image);
        }
    }

    /// Aplica as capas deste chat cujos eventos já chegaram, como depois de
    /// um lote do histórico.
    pub(super) fn settle_event_covers_in(&mut self, chat: &str) {
        let ready: Vec<String> = self
            .waiting_covers
            .keys()
            .filter(|(waiting, parent)| {
                waiting == chat && matches!(self.archive.raw(chat, parent), Ok(Some(_)))
            })
            .map(|(_, parent)| parent.clone())
            .collect();
        for parent in ready {
            self.settle_event_cover(chat, &parent);
        }
    }

    /// Aplica uma edição de evento. Ela chega cifrada com o segredo do
    /// evento, e só quem criou o evento edita. Retorna se `base` era uma
    /// edição de evento, aberta ou não.
    pub(super) fn apply_event_edit(&mut self, chat: &str, base: &wa::Message) -> bool {
        let Some(envelope) = extract_secret_encrypted(base) else {
            return false;
        };
        if envelope.kind != SecretEncKind::EventEdit {
            return false;
        }
        let Some(target) = envelope
            .target_message_key
            .id
            .clone()
            .filter(|id| !id.is_empty())
        else {
            return true;
        };
        let payload = envelope.enc_payload.to_vec();
        let iv = envelope.enc_iv.to_vec();
        let Ok(Some(existing)) = self.archive.message(chat, &target) else {
            log::debug!("event edit for an event that is not here");
            return true;
        };
        let Some(secret) = self
            .archive
            .raw(chat, &target)
            .ok()
            .flatten()
            .as_deref()
            .and_then(message_secret_from_raw)
        else {
            log::debug!("event edit without the event's secret");
            return true;
        };
        let (author, fallback) = if existing.from_me {
            let pn = self.me_pn.as_deref().and_then(Self::jid_of);
            let lid = self.me_lid.as_deref().and_then(Self::jid_of);
            match (pn, lid) {
                (Some(pn), lid) => (pn, lid),
                (None, Some(lid)) => (lid, None),
                (None, None) => return true,
            }
        } else {
            let Some(author) = Self::jid_of(&existing.sender) else {
                return true;
            };
            let fallback = self.alt_jid(&author);
            (author, fallback)
        };
        let Ok(inner) = decrypt_secret_encrypted_with_fallback(
            &payload,
            &iv,
            &secret,
            SecretEncKind::EventEdit,
            &target,
            &author,
            &author,
            fallback.as_ref(),
            fallback.as_ref(),
        ) else {
            log::debug!("could not open an event edit");
            return true;
        };
        let Some(mut edited) = content(inner.get_base_message()) else {
            return true;
        };
        // A capa vem à parte e continua a mesma.
        if let (Content::Event { cover: new, .. }, Content::Event { cover: old, .. }) =
            (&mut edited, &existing.content)
        {
            *new = old.clone();
        }
        match self
            .archive
            .set_edited_content(chat, &target, &edited, &existing.mentions)
        {
            Ok(true) => {
                self.emit_message(chat, &target);
                self.emit_chat(chat);
            }
            Ok(false) => {}
            Err(error) => log::warn!("could not store an event edit: {error}"),
        }
        true
    }

    /// Converte eventos que versões anteriores guardaram como "não suportado"
    /// e junta aos eventos as capas que ficaram como mensagens soltas.
    pub(super) fn backfill_events(&mut self) {
        const KEY: &str = "events";
        if self.archive.meta(KEY).ok().flatten().as_deref() == Some("1") {
            return;
        }
        let rows = match self.archive.unsupported_events_with_raw() {
            Ok(rows) => rows,
            Err(error) => {
                log::warn!("could not read archived events: {error}");
                return;
            }
        };
        let mut covers = Vec::new();
        let mut updated = 0;
        let mut failed = false;
        for (chat, id, raw) in rows {
            let Ok(message) = wa::Message::decode_from_slice(&raw) else {
                continue;
            };
            let base = message.get_base_message();
            if let Some((parent, image)) = cover_of(base) {
                covers.push((chat, id, parent, image));
                continue;
            }
            let Some(content) = content(base) else {
                continue;
            };
            match self.archive.set_content(&chat, &id, &content, false) {
                Ok(true) => updated += 1,
                Ok(false) => {}
                Err(error) => {
                    log::warn!("could not convert an archived event: {error}");
                    failed = true;
                }
            }
        }
        // As capas depois dos eventos, que agora já são eventos.
        for (chat, id, parent, image) in covers {
            self.file_event_cover(&chat, &parent, None, image);
            self.waiting_covers.remove(&(chat.clone(), parent.clone()));
            match self.archive.delete_message(&chat, &id) {
                Ok(_) => updated += 1,
                Err(error) => {
                    log::warn!("could not remove an archived event cover: {error}");
                    failed = true;
                }
            }
        }
        if !failed && let Err(error) = self.archive.set_meta(KEY, "1") {
            log::warn!("could not mark the event migration complete: {error}");
        }
        if updated > 0 {
            log::info!("converted {updated} archived events and covers");
            self.emit_chats();
        }
    }
}

#[cfg(test)]
mod event_tests {
    use super::*;

    fn event() -> wa::Message {
        let mut proto = wa::Message::default();
        proto.event_message = MessageField::some({
            let mut event = wa::message::EventMessage::default();
            event.name = Some("Call com o lead".into());
            event.description = Some("Apresentar a proposta".into());
            event.start_time = Some(1_800_000_000);
            event.end_time = Some(1_800_003_600);
            event.join_link = Some("https://call.whatsapp.com/video/AbCdEf123456".into());
            event.location = MessageField::some({
                let mut location = wa::message::LocationMessage::default();
                location.name = Some("Escritório Verkz".into());
                location
            });
            event
        });
        proto
    }

    #[test]
    fn an_event_becomes_a_card() {
        let Some(Content::Event {
            name,
            description,
            start,
            end,
            place,
            call_link,
            canceled,
            invite,
            cover,
        }) = classify(&event())
        else {
            panic!("an event is an event card");
        };
        assert_eq!(name, "Call com o lead");
        assert_eq!(description.as_deref(), Some("Apresentar a proposta"));
        assert_eq!(start, Some(1_800_000_000));
        assert_eq!(end, Some(1_800_003_600));
        assert_eq!(place.as_deref(), Some("Escritório Verkz"));
        assert!(call_link.is_some());
        assert!(!canceled && !invite && cover.is_none());
    }

    #[test]
    fn a_canceled_event_says_so() {
        let mut proto = event();
        proto.event_message.as_option_mut().unwrap().is_canceled = Some(true);
        assert!(matches!(
            classify(&proto),
            Some(Content::Event { canceled: true, .. })
        ));
    }

    #[test]
    fn a_shared_invite_is_an_event_card() {
        let mut proto = wa::Message::default();
        proto.event_invite_message = MessageField::some({
            let mut invite = wa::message::EventInviteMessage::default();
            invite.event_title = Some("Lançamento de inverno".into());
            invite.start_time = Some(1_800_000_000);
            invite.status = Some(wa::message::event_invite_message::EventStatus::CANCELED);
            invite
        });
        assert!(matches!(
            classify(&proto),
            Some(Content::Event {
                invite: true,
                canceled: true,
                ..
            })
        ));
    }

    fn cover() -> wa::Message {
        use wa::message_association::AssociationType;
        let mut proto = wa::Message::default();
        proto.associated_child_message = MessageField::some({
            let mut wrapper = wa::message::FutureProofMessage::default();
            wrapper.message = MessageField::some({
                let mut child = wa::Message::default();
                child.image_message = MessageField::some({
                    let mut image = wa::message::ImageMessage::default();
                    image.mimetype = Some("image/jpeg".into());
                    image.file_length = Some(2048);
                    image
                });
                child
            });
            wrapper
        });
        proto.message_context_info = MessageField::some({
            let mut context = wa::MessageContextInfo::default();
            context.message_association = MessageField::some({
                let mut association = wa::MessageAssociation::default();
                association.association_type = Some(AssociationType::EVENT_COVER_IMAGE);
                association.parent_message_key = MessageField::some({
                    let mut key = wa::MessageKey::default();
                    key.id = Some("EVENT1".into());
                    key
                });
                association
            });
            context
        });
        proto
    }

    #[test]
    fn a_cover_is_not_a_message_of_its_own() {
        assert_eq!(classify(&cover()), None);
        let (parent, image) = cover_of(cover().get_base_message()).expect("a cover");
        assert_eq!(parent, "EVENT1");
        assert_eq!(image.file_length, Some(2048));
    }

    #[test]
    fn a_cover_joins_its_event() {
        let (_, image) = cover_of(&cover()).unwrap();
        let merged = with_cover(&event().encode_to_vec(), &image).expect("merged");
        let Some(Content::Event {
            cover: Some(cover), ..
        }) = classify(&merged)
        else {
            panic!("the event shows its cover");
        };
        assert_eq!(cover.size, 2048);
        // Uma foto comum não vira capa de nada.
        assert!(with_cover(&wa::Message::text("oi").encode_to_vec(), &image).is_none());
    }
}
