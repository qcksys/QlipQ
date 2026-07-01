// View builders for `App`, include!d into main.rs (shares its imports).

impl App {
    fn top_bar(&self) -> Element<'_, Message> {
        let pending = self
            .items
            .iter()
            .filter(|i| i.status != QueueStatus::Done && !item_dismissed(i))
            .count();
        let brand = text("QlipQ")
            .size(theme::DISPLAY)
            .font(theme::FONT_BOLD)
            .style(|t: &Theme| text::Style { color: Some(t.extended_palette().primary.base.color) });
        let bar = row![
            brand,
            Space::new().width(Length::Fill),
            button(text(format!("Queue ({pending})")).size(theme::LABEL)).style(theme::nav(matches!(self.view, View::Queue))).on_press(Message::ShowQueue),
            button(text("Settings").size(theme::LABEL)).style(theme::nav(matches!(self.view, View::Settings))).on_press(Message::ShowSettings),
            button(text("GitHub").size(theme::LABEL)).style(theme::btn_ghost).on_press(Message::OpenRepo),
        ]
        .spacing(theme::SM)
        .align_y(iced::Alignment::Center)
        .padding([theme::SM, theme::LG]);
        container(bar).width(Length::Fill).style(theme::top_bar).into()
    }

    fn queue_sidebar(&self) -> Element<'_, Message> {
        let mut all_tags: Vec<String> = self.items.iter().flat_map(|i| i.tags.clone().unwrap_or_default()).collect();
        all_tags.sort();
        all_tags.dedup();
        all_tags.retain(|t| t != DISMISSED_TAG);

        let visible: Vec<&QueueItem> = self
            .items
            .iter()
            .filter(|i| match &self.tag_filter {
                Some(f) if all_tags.contains(f) => i.tags.as_ref().map(|t| t.contains(f)).unwrap_or(false),
                _ => !item_dismissed(i),
            })
            .collect();

        let mut col = column![].spacing(theme::SM).padding(theme::MD);

        // Keep the rescan button + tag filters in one always-present header child so the queue
        // scrollable below stays at a fixed index in `col`. iced tracks a scrollable's offset by its
        // position in the widget tree; if a conditional sibling above it vanished (e.g. deleting the
        // last tagged clip drops the filter row), the scrollable would shift index and reset to the
        // top — the exact scroll jump we're avoiding on delete.
        let mut header = column![].spacing(theme::SM);
        if !self.config.watched_folders.is_empty() {
            header = header.push(button(text("Rescan all folders").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::RescanAll));
        }
        if !all_tags.is_empty() {
            let mut filters = row![button(text("All").size(theme::SMALL)).style(theme::nav(self.tag_filter.is_none())).on_press(Message::SetTagFilter(None))].spacing(theme::XS);
            for t in &all_tags {
                let active = self.tag_filter.as_deref() == Some(t.as_str());
                filters = filters.push(button(text(format!("#{t}")).size(theme::SMALL)).style(theme::nav(active)).on_press(Message::SetTagFilter(Some(t.clone()))));
            }
            header = header.push(filters);
        }
        col = col.push(header);

        if visible.is_empty() {
            let no_folders = self.config.watched_folders.is_empty();
            let msg = if no_folders { "No watched folders yet." } else { "Queue is empty. New recordings show up here automatically." };
            let mut empty = column![text(msg).size(theme::LABEL).style(|t| text::Style { color: Some(theme::muted(t)) })]
                .spacing(theme::MD)
                .align_x(iced::Alignment::Center);
            if no_folders {
                empty = empty.push(button(text("Add a watched folder").size(theme::LABEL)).style(theme::btn_primary).on_press(Message::ShowSettings));
            }
            col = col.push(container(empty).width(Length::Fill).height(Length::Fill).padding(theme::XL).center_x(Length::Fill).center_y(Length::Fill));
        } else {
            // Right padding keeps the cards (and their Delete buttons) clear of the scrollbar.
            let mut list = column![].spacing(theme::SM).padding(iced::Padding::from(0.0).right(theme::MD));
            for item in visible {
                list = list.push(self.queue_card(item));
            }
            col = col.push(scrollable(list).height(Length::Fill));
        }

        container(col).into()
    }

    fn queue_card(&self, item: &QueueItem) -> Element<'_, Message> {
        let selected = self.selected_id.as_deref() == Some(&item.id);
        let status = item.status;
        let header = row![
            container(Space::new().width(Length::Fixed(8.0)).height(Length::Fixed(8.0))).style(theme::status_dot(status)),
            text(item.file_name.clone()).size(theme::BODY).font(theme::FONT_MEDIUM).width(Length::Fill),
        ]
        .spacing(theme::SM)
        .align_y(iced::Alignment::Center);
        let open = button(
            column![
                header,
                text(meta_line(item)).size(theme::META).style(|t| text::Style { color: Some(theme::muted(t)) }),
                text(status_label(status)).size(theme::SMALL).font(theme::FONT_MEDIUM).style(move |t| text::Style { color: Some(theme::status_color(t, status)) }),
            ]
            .spacing(theme::XS),
        )
        .width(Length::Fill)
        .padding([theme::XS, 0.0])
        .style(theme::btn_plain)
        .on_press(Message::SelectItem(item.id.clone()));

        let mut card = column![open].spacing(theme::SM);

        let tags: Vec<String> = item.tags.clone().unwrap_or_default().into_iter().filter(|t| t != DISMISSED_TAG).collect();
        if !tags.is_empty() {
            let mut tags_row = row![].spacing(theme::XS);
            for t in tags {
                tags_row = tags_row.push(chip(format!("#{t}")));
            }
            card = card.push(tags_row);
        }

        let actions = row![
            button(text("Rename").size(theme::SMALL)).style(theme::btn_secondary).on_press(Message::RenameOpen(item.id.clone())),
            with_tip(
                button(text("Open").size(theme::SMALL)).style(theme::btn_secondary).on_press(Message::RevealItem(item.path.clone())).into(),
                "Show in file explorer".to_string(),
            ),
            button(text(if item_dismissed(item) { "Restore" } else { "Dismiss" }).size(theme::SMALL)).style(theme::btn_secondary).on_press(Message::Dismiss(item.id.clone())),
            Space::new().width(Length::Fill),
            button(text("Delete").size(theme::SMALL)).style(theme::btn_danger).on_press(Message::RequestDelete(item.id.clone())),
        ]
        .spacing(theme::XS);
        card = card.push(actions);

        let hovered = self.hovered_card.as_deref() == Some(item.id.as_str());
        mouse_area(container(card).padding(theme::SM).style(theme::queue_card(selected, hovered)))
            .on_enter(Message::HoverCard(item.id.clone()))
            .on_exit(Message::HoverLeave(item.id.clone()))
            .into()
    }

    fn editor_view(&self) -> Element<'_, Message> {
        let Some(ed) = &self.editor else {
            return empty_state("Select a clip from the queue to start editing.");
        };
        if let Some(err) = &ed.load_error {
            return container(
                column![
                    text("Could not read this clip.").size(theme::TITLE).font(theme::FONT_SEMIBOLD),
                    text(err.clone()).size(theme::META).style(|t| text::Style { color: Some(theme::muted(t)) }),
                ]
                .spacing(theme::SM),
            )
            .padding(theme::XL)
            .into();
        }
        let Some(media) = &ed.media else {
            return empty_state("Reading clip…");
        };
        let Some(item) = self.items.iter().find(|i| i.id == ed.item_id) else {
            return container(text("…")).into();
        };

        // Preview pane (scalable; double-click toggles fullscreen) + a zoom / fullscreen toolbar.
        let preview = preview_pane(ed, media, Length::Fixed(360.0 * self.preview_scale));
        let zoom = (self.preview_scale * 100.0).round() as i32;
        let preview_tools = row![
            Space::new().width(Length::Fill),
            button(text("−").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::PreviewZoom(-0.25)),
            text(format!("{zoom}%")).size(theme::META).font(Font::MONOSPACE).width(Length::Fixed(44.0)).style(|t| text::Style { color: Some(theme::muted(t)) }),
            button(text("+").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::PreviewZoom(0.25)),
            button(text("Fullscreen").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::ToggleFullscreen),
        ]
        .spacing(theme::XS)
        .align_y(iced::Alignment::Center);

        let transport = container(transport_row(ed)).width(Length::Fill).center_x(Length::Fill);

        // Timeline.
        let dur = media.duration_sec.max(0.001);
        let scrub = slider(0.0..=dur, ed.current_time.min(dur), Message::Seek).step(0.05).style(theme::slider_style);
        let time_row = row![
            text_input("0:00.000", &ed.time_input)
                .on_input(Message::TimestampEdited)
                .on_submit(Message::TimestampSubmit)
                .font(Font::MONOSPACE)
                .style(theme::input)
                .width(Length::Fixed(110.0)),
            text(format!("/ {}", format_timestamp(dur))).size(theme::META).font(Font::MONOSPACE).style(|t| text::Style { color: Some(theme::muted(t)) }),
        ]
        .spacing(theme::SM)
        .align_y(iced::Alignment::Center);
        let inout_row = row![
            text(format!("In {}", format_timestamp(ed.trim_start))).size(theme::META).font(Font::MONOSPACE).style(|t| text::Style { color: Some(theme::muted(t)) }),
            button(text("Set in").size(theme::SMALL)).style(theme::btn_secondary).on_press(Message::SetIn),
            text(datetimes::format_duration(ed.trim_end - ed.trim_start)).size(theme::LABEL).font(theme::FONT_MEDIUM),
            button(text("Set out").size(theme::SMALL)).style(theme::btn_secondary).on_press(Message::SetOut),
            text(format!("Out {}", format_timestamp(ed.trim_end))).size(theme::META).font(Font::MONOSPACE).style(|t| text::Style { color: Some(theme::muted(t)) }),
        ]
        .spacing(theme::SM)
        .align_y(iced::Alignment::Center);
        let timeline = column![scrub, time_row, inout_row].spacing(theme::SM);

        // Options laid out in two columns: media edits (crop, audio) on the left, output + metadata
        // (quality override, tags) on the right. The two toggle cards (Crop, Override) head each column.
        let options = row![
            column![self.crop_section(ed, media), self.audio_section(ed)].spacing(theme::MD).width(Length::Fill),
            column![self.override_section(item), self.editor_tags(item)].spacing(theme::MD).width(Length::Fill),
        ]
        .spacing(theme::MD);

        // Export bar.
        let spec = editor_spec(ed);
        let validation = qlipq_core::edit_spec::validate_edit_spec(&spec, media);
        let encode = output_settings_to_encode(&self.effective_output(item), media);
        let estimate = estimate_export_size(media, &spec, &encode);
        let dims = if ed.crop_enabled {
            format!("{}×{}", ed.crop.width, ed.crop.height)
        } else {
            format!("{}×{}", media.width, media.height)
        };
        let mut stats = row![
            stat("Duration", datetimes::format_duration(qlipq_core::edit_spec::effective_duration(&spec, media))),
            stat("Resolution", dims),
            stat("Est. size", format!("{}{}", if estimate.approximate { "≈" } else { "" }, format_bytes(estimate.bytes))),
        ]
        .spacing(theme::XL)
        .align_y(iced::Alignment::Center);
        if let Some(err) = &validation {
            stats = stats.push(text(err.clone()).size(theme::LABEL).style(|t: &Theme| text::Style { color: Some(t.extended_palette().danger.base.color) }));
        }
        if ed.exporting {
            stats = stats.push(container(progress_bar(0.0..=1.0, ed.progress_display).style(theme::progress_style)).width(Length::Fixed(160.0)));
            stats = stats.push(button(text("Cancel").size(theme::LABEL)).style(theme::btn_danger).on_press(Message::CancelExport));
        }

        let mut export_bar = row![stats, Space::new().width(Length::Fill)].align_y(iced::Alignment::Center).spacing(theme::SM);
        if item.export_path.is_some() && !ed.exporting {
            export_bar = export_bar.push(button(text("Show file").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::ShowExported));
        }
        let can_export = validation.is_none() && !ed.exporting && !self.config.output_folder.is_empty();
        let export_btn = button(
            text(if ed.exporting { format!("Exporting {}%", (ed.progress_display * 100.0) as i32) } else { "Export clip".to_string() })
                .size(theme::BODY)
                .font(theme::FONT_MEDIUM),
        )
        .style(theme::btn_primary);
        let export_el: Element<Message> = if can_export {
            export_btn.on_press(Message::Export).into()
        } else if self.config.output_folder.is_empty() {
            with_tip(export_btn.into(), "Set an output folder in Settings".to_string())
        } else if let Some(err) = &validation {
            with_tip(export_btn.into(), err.clone())
        } else {
            export_btn.into()
        };
        export_bar = export_bar.push(export_el);

        let player_zone = column![preview_tools, preview, transport, timeline].spacing(theme::MD);

        let mut body = column![player_zone, rule::horizontal(1), options, rule::horizontal(1), export_bar]
            .spacing(theme::LG)
            .padding(theme::LG);
        if self.config.debug {
            body = body.push(rule::horizontal(1));
            body = body.push(self.debug_section(ed, media, item));
        }
        scrollable(body).into()
    }

    /// Opt-in developer panel (Settings → Playback → *Show debug panel*): the clip's media details,
    /// the active decoder, and — while playing — live preview buffer stats, so a stuttering preview's
    /// cause is visible (software decode + a starving video/audio buffer).
    fn debug_section<'a>(&self, ed: &'a Editor, media: &MediaInfo, item: &'a QueueItem) -> Element<'a, Message> {
        let container_ext = item.path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_else(|| "?".into());
        let bitrate = match (item.file_size_bytes, media.duration_sec) {
            (Some(b), d) if d > 0.0 => format!("{:.1} Mbps", (b as f64 * 8.0) / d / 1_000_000.0),
            _ => "—".to_string(),
        };
        let decoder = match ed.decode_hw {
            Some(true) => "D3D11VA (GPU)".to_string(),
            Some(false) => "software (CPU)".to_string(),
            None => "—".to_string(),
        };
        let preview_res = match ed.preview_dims {
            Some((w, h)) => format!("{w}×{h} · {}", if ed.is_hdr { "libplacebo HDR→SDR" } else { "scale" }),
            None => "—".to_string(),
        };

        // Static clip/decoder facts, then live playback health (only while the player is running).
        let mut pairs: Vec<(&'static str, String)> = vec![
            ("File", item.file_name.clone()),
            ("Path", item.path.clone()),
            ("Container", container_ext),
            ("Video", format!("{} · {}×{} · {:.3} fps{}", media.video_codec, media.width, media.height, media.fps, if ed.is_hdr { " · HDR" } else { "" })),
            ("Source bitrate", bitrate),
            ("Audio tracks", format!("{}", media.audio_streams.len())),
            ("Decoder", decoder),
            ("Preview", preview_res),
        ];
        let live_from = pairs.len();
        if let Some(p) = &ed.player {
            pairs.push(("Master clock", if p.audio_clock() { "audio-synced".into() } else { "wall clock".into() }));
            pairs.push(("Video buffer", format!("{}/{} frames", p.queue_depth(), libav::VIDEO_LOOKAHEAD)));
            pairs.push(("Dropped frames", format!("{} (preview display only)", p.dropped_frames())));
            if p.audio_clock() {
                pairs.push(("Audio buffer", format!("{}% full", (p.audio_fill() * 100.0) as i32)));
                pairs.push(("Audio underruns", format!("{}", p.audio_underruns())));
            }
        } else {
            pairs.push(("Playback", "paused (play for live buffer stats)".into()));
        }

        let mut rows = column![].spacing(theme::XS);
        for (i, &(label, ref value)) in pairs.iter().enumerate() {
            if i == live_from {
                rows = rows.push(rule::horizontal(1)); // divider between static facts and live stats
            }
            rows = rows.push(dbg_row(label, value.clone()));
        }

        // The single most common cause of preview stutter — call it out in plain language.
        if ed.decode_hw == Some(false) {
            rows = rows.push(
                text("No hardware decoder for this codec/GPU — software decode may not keep up with heavy 1440p/4K AV1/HEVC, which stutters the preview. Exports are unaffected.")
                    .size(theme::SMALL)
                    .style(|t: &Theme| text::Style { color: Some(t.extended_palette().warning.base.color) }),
            );
        }

        // iced text labels aren't selectable, so offer a one-click copy of the whole panel as text.
        let blob = pairs.iter().map(|(l, v)| format!("{l}: {v}")).collect::<Vec<_>>().join("\n");
        let header = row![
            text("Debug").size(theme::HEADING).font(theme::FONT_SEMIBOLD).width(Length::Fill),
            button(text("Copy").size(theme::SMALL)).style(theme::btn_secondary).on_press(Message::CopyText(blob)),
        ]
        .align_y(iced::Alignment::Center);

        container(column![header, rows].spacing(theme::SM))
            .width(Length::Fill)
            .padding(theme::MD)
            .style(theme::card)
            .into()
    }

    /// The preview expanded to fill the window, with a transport + exit bar pinned to the bottom.
    fn fullscreen_view(&self) -> Element<'_, Message> {
        let Some(ed) = &self.editor else { return empty_state("…") };
        let Some(media) = &ed.media else { return empty_state("…") };
        let bar = container(
            row![
                container(transport_row(ed)).width(Length::Fill).center_x(Length::Fill),
                button(text("Exit fullscreen").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::ToggleFullscreen),
            ]
            .spacing(theme::SM)
            .align_y(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .padding([theme::SM, theme::LG])
        .style(theme::top_bar);
        container(column![preview_pane(ed, media, Length::Fill), bar])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::canvas)
            .into()
    }

    fn crop_section<'a>(&self, ed: &'a Editor, media: &MediaInfo) -> Element<'a, Message> {
        let mut col = column![
            text("Crop").size(theme::HEADING).font(theme::FONT_SEMIBOLD),
            checkbox(ed.crop_enabled)
                .label(format!("Enable crop ({}×{} source)", media.width, media.height))
                .text_size(theme::LABEL)
                .style(theme::checkbox_style)
                .on_toggle(Message::ToggleCrop),
        ]
        .spacing(theme::SM);
        if ed.crop_enabled {
            col = col.push(
                row![
                    num_field("X", ed.crop.x, |s| Message::CropEdited(0, s)),
                    num_field("Y", ed.crop.y, |s| Message::CropEdited(1, s)),
                    num_field("W", ed.crop.width, |s| Message::CropEdited(2, s)),
                    num_field("H", ed.crop.height, |s| Message::CropEdited(3, s)),
                ]
                .spacing(theme::SM),
            );
        }
        container(col).width(Length::Fill).padding(theme::MD).style(theme::card).into()
    }

    fn audio_section<'a>(&self, ed: &'a Editor) -> Element<'a, Message> {
        let mut col = column![text("Audio tracks").size(theme::HEADING).font(theme::FONT_SEMIBOLD)].spacing(theme::SM);
        if ed.audio.is_empty() {
            col = col.push(text("No audio tracks in this clip.").size(theme::META).style(|t| text::Style { color: Some(theme::muted(t)) }));
        }
        for r in &ed.audio {
            let idx = r.index;
            col = col.push(
                column![
                    row![
                        checkbox(r.enabled).label(r.label.clone()).text_size(theme::LABEL).style(theme::checkbox_style).on_toggle(move |on| Message::AudioToggle(idx, on)),
                        text(r.detail.clone()).size(theme::SMALL).style(|t| text::Style { color: Some(theme::muted(t)) }),
                    ]
                    .spacing(theme::SM)
                    .align_y(iced::Alignment::Center),
                    row![
                        container(slider(0.0..=2.0, r.volume, move |v| Message::AudioVolume(idx, v)).step(0.05).on_release(Message::AudioVolumeCommit).style(theme::slider_style))
                            .width(Length::Fixed(180.0)),
                        text(format!("{}%", (r.volume * 100.0) as i32)).size(theme::SMALL).font(Font::MONOSPACE).style(|t| text::Style { color: Some(theme::muted(t)) }),
                    ]
                    .spacing(theme::SM)
                    .align_y(iced::Alignment::Center),
                ]
                .spacing(theme::XS),
            );
        }
        container(col).width(Length::Fill).padding(theme::MD).style(theme::card).into()
    }

    fn override_section(&self, item: &QueueItem) -> Element<'_, Message> {
        let enabled = item.output_override.is_some();
        let mut col = column![checkbox(enabled).label("Override quality for this clip").text_size(theme::LABEL).style(theme::checkbox_style).on_toggle(Message::ToggleOverride)].spacing(theme::SM);
        if enabled {
            let out = self.effective_output(item);
            let mut fields = row![pick_list(QmChoice::ALL.to_vec(), Some(QmChoice::from_core(out.quality_mode)), Message::OverrideQm).style(theme::pick_list_style)].spacing(theme::SM);
            match out.quality_mode {
                QualityMode::Preset => {
                    fields = fields.push(pick_list(QpChoice::ALL.to_vec(), Some(QpChoice::from_core(out.quality_preset)), Message::OverrideQp).style(theme::pick_list_style));
                }
                QualityMode::Crf | QualityMode::Vbr => {
                    fields = fields.push(num_field("CRF", out.crf, Message::OverrideCrf));
                    if out.quality_mode == QualityMode::Vbr {
                        fields = fields.push(num_field("Max kbps", out.video_bitrate_kbps, Message::OverrideBitrate));
                    }
                }
                QualityMode::Bitrate => {
                    fields = fields.push(num_field("Video kbps", out.video_bitrate_kbps, Message::OverrideBitrate));
                }
            }
            col = col.push(fields);
        }
        container(col).width(Length::Fill).padding(theme::MD).style(theme::card).into()
    }

    fn editor_tags(&self, item: &QueueItem) -> Element<'_, Message> {
        let mut tags_row = row![].spacing(theme::SM);
        for t in item.tags.clone().unwrap_or_default() {
            if t == DISMISSED_TAG {
                continue;
            }
            let tag = t.clone();
            tags_row = tags_row.push(removable_chip(t, Message::RemoveTag(tag)));
        }
        let input = text_input("Add tag…", &self.new_tag).on_input(Message::NewTagChanged).on_submit(Message::AddTag).style(theme::input).width(Length::Fixed(160.0));
        container(column![
            text("Tags").size(theme::HEADING).font(theme::FONT_SEMIBOLD),
            row![tags_row, input].spacing(theme::SM).align_y(iced::Alignment::Center),
        ]
        .spacing(theme::SM))
        .width(Length::Fill)
        .padding(theme::MD)
        .style(theme::card)
        .into()
    }

    fn settings_view(&self) -> Element<'_, Message> {
        let out = &self.config.output;

        // Watched folders.
        let mut folders = column![].spacing(theme::SM);
        for f in &self.config.watched_folders {
            folders = folders.push(
                row![
                    text(f.clone()).size(theme::LABEL).width(Length::Fill),
                    button(text("Reprocess").size(theme::SMALL)).style(theme::btn_secondary).on_press(Message::Reprocess(f.clone())),
                    button(text("Remove").size(theme::SMALL)).style(theme::btn_ghost).on_press(Message::RemoveFolder(f.clone())),
                ]
                .spacing(theme::SM)
                .align_y(iced::Alignment::Center),
            );
        }
        let mut add_row = row![button(text("Add folder…").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::PickFolder(PickPurpose::WatchedFolder))].spacing(theme::SM);
        if let Some(obs) = &self.presets.obs {
            if !self.config.watched_folders.contains(obs) {
                add_row = add_row.push(button(text(format!("+ OBS ({obs})")).size(theme::SMALL)).style(theme::btn_ghost).on_press(Message::AddPreset(obs.clone())));
            }
        }
        if let Some(nv) = &self.presets.nvidia_share {
            if !self.config.watched_folders.contains(nv) {
                add_row = add_row.push(button(text(format!("+ NVIDIA Share ({nv})")).size(theme::SMALL)).style(theme::btn_ghost).on_press(Message::AddPreset(nv.clone())));
            }
        }
        folders = folders.push(add_row);

        // Output folder.
        let output_folder = row![
            text_input("Where exported clips are saved", &self.config.output_folder).on_input(Message::OutputFolderChanged).style(theme::input).width(Length::Fill),
            button(text("Browse…").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::PickFolder(PickPurpose::OutputFolder)),
        ]
        .spacing(theme::SM);

        // Output defaults.
        let mut quality = row![pick_list(QmChoice::ALL.to_vec(), Some(QmChoice::from_core(out.quality_mode)), Message::SetQm).style(theme::pick_list_style)].spacing(theme::SM);
        match out.quality_mode {
            QualityMode::Preset => quality = quality.push(pick_list(QpChoice::ALL.to_vec(), Some(QpChoice::from_core(out.quality_preset)), Message::SetQp).style(theme::pick_list_style)),
            QualityMode::Crf | QualityMode::Vbr => {
                quality = quality.push(num_field("CRF", out.crf, Message::SetCrf));
                if out.quality_mode == QualityMode::Vbr {
                    quality = quality.push(num_field("Max kbps", out.video_bitrate_kbps, Message::SetBitrate));
                }
            }
            QualityMode::Bitrate => quality = quality.push(num_field("Video kbps", out.video_bitrate_kbps, Message::SetBitrate)),
        }
        let encoder_options: Vec<String> = ENCODER_PRESETS.iter().map(|s| s.to_string()).collect();
        let encode_row = row![
            pick_list(encoder_options, Some(out.encoder_preset.clone()), Message::SetEncoder).style(theme::pick_list_style),
            pick_list(CodecChoice::ALL.to_vec(), Some(CodecChoice::from_core(out.video_codec)), Message::SetCodec).style(theme::pick_list_style),
            pick_list(ContainerChoice::ALL.to_vec(), Some(ContainerChoice::from_core(out.container)), Message::SetContainer).style(theme::pick_list_style),
        ]
        .spacing(theme::SM);
        let rate_row = row![
            pick_list(FpsChoice::ALL.to_vec(), Some(FpsChoice::from_core(out.fps)), Message::SetFps).style(theme::pick_list_style),
            pick_list(ResChoice::ALL.to_vec(), Some(ResChoice::from_core(out.max_height)), Message::SetRes).style(theme::pick_list_style),
            pick_list(AudioKbpsChoice::ALL.to_vec(), Some(AudioKbpsChoice::from_core(out.audio_bitrate_kbps)), Message::SetAudioKbps).style(theme::pick_list_style),
        ]
        .spacing(theme::SM);

        // After export.
        let ae = &self.config.after_export;
        let mut after = column![pick_list(AfterChoice::ALL.to_vec(), Some(AfterChoice::from_core(ae.action)), Message::SetAfter).style(theme::pick_list_style)].spacing(theme::SM);
        if ae.action == AfterExportAction::Move {
            after = after.push(
                row![
                    text_input("Destination folder", &ae.move_folder).on_input(Message::MoveFolderChanged).style(theme::input).width(Length::Fill),
                    button(text("Browse…").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::PickFolder(PickPurpose::MoveFolder)),
                ]
                .spacing(theme::SM),
            );
        }
        if ae.action == AfterExportAction::Rename {
            after = after.push(
                row![
                    text_input("Prefix", &ae.rename_prefix).on_input(Message::RenamePrefixChanged).style(theme::input),
                    text_input("Suffix", &ae.rename_suffix).on_input(Message::RenameSuffixChanged).style(theme::input),
                ]
                .spacing(theme::SM),
            );
        }

        // HDR preview brightness — a gamma lift for HDR clips that tonemap too dark in the preview.
        let gamma = self.config.hdr_preview_gamma;
        let hdr_preview = column![
            row![
                text("Brightness").size(theme::LABEL).width(Length::Fill),
                text(format!("gamma {gamma:.2}")).size(theme::SMALL).style(|t| text::Style { color: Some(theme::muted(t)) }),
            ]
            .align_y(iced::Alignment::Center),
            slider(1.0..=3.0, gamma, Message::SetHdrPreviewGamma)
                .step(0.05)
                .on_release(Message::ApplyHdrPreviewGamma)
                .style(theme::slider_style),
            text("Brightens HDR clips that preview too dark (HDR sources only; preview only — exports are unaffected). 1.0 = off.")
                .size(theme::SMALL)
                .style(|t| text::Style { color: Some(theme::muted(t)) }),
        ]
        .spacing(theme::XS);

        // Playback + developer toggles.
        let playback = column![
            checkbox(self.config.autoplay)
                .label("Play clips automatically when selected")
                .text_size(theme::LABEL)
                .style(theme::checkbox_style)
                .on_toggle(Message::ToggleAutoplay),
            checkbox(self.config.debug)
                .label("Show debug panel in the editor")
                .text_size(theme::LABEL)
                .style(theme::checkbox_style)
                .on_toggle(Message::ToggleDebug),
            text("The debug panel shows the clip's details, the active decoder (hardware vs software), and live preview buffer stats to help diagnose playback stutter.")
                .size(theme::SMALL)
                .style(|t| text::Style { color: Some(theme::muted(t)) }),
        ]
        .spacing(theme::SM);

        // Preview quality (resolution the preview decodes/tonemaps at — separate from export size).
        let preview_quality = column![
            pick_list(PreviewResChoice::ALL.to_vec(), Some(PreviewResChoice::from_core(self.config.preview_max_height)), Message::SetPreviewRes).style(theme::pick_list_style),
            text("Resolution the preview decodes at (never upscales past the source). Higher is sharper but costs more decode/GPU work — lower it if playback stutters. Preview only; exports are unaffected.")
                .size(theme::SMALL)
                .style(|t| text::Style { color: Some(theme::muted(t)) }),
        ]
        .spacing(theme::XS);

        let body = column![
            text("Settings").size(theme::DISPLAY).font(theme::FONT_BOLD),
            section("Watched folders", folders.into()),
            section("Output folder", output_folder.into()),
            section("Output defaults", column![quality, encode_row, rate_row].spacing(theme::SM).into()),
            section("Preview quality", preview_quality.into()),
            section("Playback", playback.into()),
            section(
                "Naming template",
                column![
                    text_input("{date}_{source}_{name}", &self.config.naming_template).on_input(Message::NamingChanged).style(theme::input),
                    text("Tokens: {date} {time} {datetime} {source} {name} {index}").size(theme::SMALL).style(|t| text::Style { color: Some(theme::muted(t)) }),
                ]
                .spacing(theme::XS)
                .into(),
            ),
            section("HDR preview", hdr_preview.into()),
            section("After export", after.into()),
            section("Editor shortcuts (Premiere Pro defaults)", self.keybinds_section()),
            button(text("Open config file").size(theme::LABEL)).style(theme::btn_ghost).on_press(Message::OpenConfigFile),
        ]
        .spacing(theme::LG)
        .padding(theme::XL);

        scrollable(container(body).max_width(760.0).center_x(Length::Fill)).into()
    }

    fn keybinds_section(&self) -> Element<'_, Message> {
        let kb = &self.config.keybinds;
        column![
            kb_row("Play / pause", &kb.play_pause, KbField::PlayPause),
            kb_row("Set in", &kb.set_in, KbField::SetIn),
            kb_row("Set out", &kb.set_out, KbField::SetOut),
            kb_row("Frame back", &kb.frame_back, KbField::FrameBack),
            kb_row("Frame forward", &kb.frame_forward, KbField::FrameForward),
            kb_row("Jump back 5s", &kb.jump_back, KbField::JumpBack),
            kb_row("Jump forward 5s", &kb.jump_forward, KbField::JumpForward),
            kb_row("Go to start", &kb.go_to_start, KbField::GoToStart),
            kb_row("Go to end", &kb.go_to_end, KbField::GoToEnd),
            kb_row("Export", &kb.export, KbField::Export),
            text("Combos like \"Space\", \"I\", \"Shift+Left\", \"Ctrl+M\" (modifiers Ctrl/Shift/Alt/Cmd). Also editable in config.json.")
                .size(theme::SMALL)
                .style(|t| text::Style { color: Some(theme::muted(t)) }),
        ]
        .spacing(theme::XS)
        .into()
    }

    // ---- Modals ----

    fn rename_modal<'a>(&'a self, r: &'a RenameState) -> Element<'a, Message> {
        modal(
            column![
                text("Rename recording").size(theme::TITLE).font(theme::FONT_SEMIBOLD),
                text_input("name", &r.value).on_input(Message::RenameValue).on_submit(Message::RenameConfirm).style(theme::input),
                row![
                    button(text("Use template").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::RenameTemplate),
                    Space::new().width(Length::Fill),
                    button(text("Cancel").size(theme::LABEL)).style(theme::btn_ghost).on_press(Message::RenameCancel),
                    button(text("Rename").size(theme::LABEL).font(theme::FONT_MEDIUM)).style(theme::btn_primary).on_press(Message::RenameConfirm),
                ]
                .spacing(theme::SM)
                .align_y(iced::Alignment::Center),
            ]
            .spacing(theme::MD),
            Message::RenameCancel,
        )
    }

    fn delete_modal(&self, id: &str) -> Element<'_, Message> {
        let name = self.items.iter().find(|i| i.id == id).map(|i| i.file_name.clone()).unwrap_or_default();
        modal(
            column![
                text("Delete this file from disk?").size(theme::TITLE).font(theme::FONT_SEMIBOLD),
                text(format!("{name} will be permanently deleted. This can't be undone.")).size(theme::LABEL).style(|t| text::Style { color: Some(theme::muted(t)) }),
                row![
                    Space::new().width(Length::Fill),
                    button(text("Cancel").size(theme::LABEL)).style(theme::btn_ghost).on_press(Message::DeleteCancel),
                    button(text("Delete").size(theme::LABEL).font(theme::FONT_MEDIUM)).style(theme::btn_danger).on_press(Message::DeleteConfirm),
                ]
                .spacing(theme::SM)
                .align_y(iced::Alignment::Center),
            ]
            .spacing(theme::MD),
            Message::DeleteCancel,
        )
    }

    fn delete_error_modal<'a>(&self, msg: &'a str) -> Element<'a, Message> {
        modal(
            column![
                text("Couldn't delete file").size(theme::TITLE).font(theme::FONT_SEMIBOLD),
                text(msg.to_string()).size(theme::LABEL).style(|t| text::Style { color: Some(theme::muted(t)) }),
                row![
                    Space::new().width(Length::Fill),
                    button(text("OK").size(theme::LABEL).font(theme::FONT_MEDIUM)).style(theme::btn_primary).on_press(Message::DismissModal),
                ]
                .spacing(theme::SM)
                .align_y(iced::Alignment::Center),
            ]
            .spacing(theme::MD),
            Message::DismissModal,
        )
    }

    fn overwrite_modal(&self, target: &str) -> Element<'_, Message> {
        modal(
            column![
                text("Overwrite existing file?").size(theme::TITLE).font(theme::FONT_SEMIBOLD),
                text(format!("A file already exists at {target}. Exporting will replace it.")).size(theme::LABEL).style(|t| text::Style { color: Some(theme::muted(t)) }),
                row![
                    button(text("Cancel").size(theme::LABEL)).style(theme::btn_ghost).on_press(Message::Overwrite(2)),
                    Space::new().width(Length::Fill),
                    button(text("Append timestamp").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::Overwrite(1)),
                    button(text("Overwrite").size(theme::LABEL).font(theme::FONT_MEDIUM)).style(theme::btn_primary).on_press(Message::Overwrite(0)),
                ]
                .spacing(theme::SM)
                .align_y(iced::Alignment::Center),
            ]
            .spacing(theme::MD),
            Message::Overwrite(2),
        )
    }

    fn after_modal(&self) -> Element<'_, Message> {
        modal(
            column![
                text("Export complete").size(theme::TITLE).font(theme::FONT_SEMIBOLD),
                text("What should happen to the original recording?").size(theme::LABEL).style(|t| text::Style { color: Some(theme::muted(t)) }),
                row![
                    button(text("Keep").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::AfterChoice(AfterExportAction::Nothing)),
                    button(text("Rename").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::AfterChoice(AfterExportAction::Rename)),
                    button(text("Move…").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::AfterChoice(AfterExportAction::Move)),
                    Space::new().width(Length::Fill),
                    button(text("Delete").size(theme::LABEL).font(theme::FONT_MEDIUM)).style(theme::btn_danger).on_press(Message::AfterChoice(AfterExportAction::Delete)),
                ]
                .spacing(theme::SM)
                .align_y(iced::Alignment::Center),
            ]
            .spacing(theme::MD),
            Message::DismissModal,
        )
    }
}

// ---- free helpers ----

fn item_dismissed(item: &QueueItem) -> bool {
    item.tags.as_ref().map(|t| t.iter().any(|x| x == DISMISSED_TAG)).unwrap_or(false)
}

fn status_label(status: QueueStatus) -> &'static str {
    match status {
        QueueStatus::Pending => "Pending",
        QueueStatus::Ready => "Ready",
        QueueStatus::Editing => "Editing",
        QueueStatus::Exporting => "Exporting",
        QueueStatus::Done => "Done",
        QueueStatus::Error => "Error",
    }
}

fn meta_line(item: &QueueItem) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(s) = &item.source {
        parts.push(s.clone());
    }
    let when = item.recorded_at.as_deref().or(item.file_modified_at.as_deref()).and_then(iso::to_local);
    match when {
        Some(w) => parts.push(format!("{} {}", datetimes::format_date(&w), datetimes::format_time(&w))),
        None => parts.push("Unknown time".to_string()),
    }
    if let Some(d) = item.duration_sec {
        parts.push(datetimes::format_duration(d));
    }
    if let Some(b) = item.file_size_bytes {
        parts.push(format_bytes(b as f64));
    }
    parts.join(" · ")
}

/// The −60/−5/−1/Play/+1/+5/+60 transport buttons (shared by the editor and the fullscreen bar).
fn transport_row(ed: &Editor) -> Element<'static, Message> {
    row![
        button(text("−60s").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::Skip(-60.0)),
        button(text("−5s").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::Skip(-5.0)),
        button(text("−1s").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::Skip(-1.0)),
        button(text(if ed.playing { "Pause" } else { "Play" }).size(theme::LABEL).font(theme::FONT_MEDIUM)).style(theme::btn_primary).on_press(Message::TogglePlay),
        button(text("+1s").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::Skip(1.0)),
        button(text("+5s").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::Skip(5.0)),
        button(text("+60s").size(theme::LABEL)).style(theme::btn_secondary).on_press(Message::Skip(60.0)),
    ]
    .spacing(theme::XS)
    .align_y(iced::Alignment::Center)
    .into()
}

/// The video preview: the wgpu shader fitted (aspect-preserving) into `height` of available width,
/// framed in a panel. Double-clicking toggles fullscreen.
fn preview_pane<'a>(ed: &'a Editor, media: &MediaInfo, height: Length) -> Element<'a, Message> {
    let vw = media.width.max(1) as f32;
    let vh = media.height.max(1) as f32;
    let frame = ed.shared_frame.clone();
    let has_frame = ed.has_frame;
    let video = responsive(move |bounds| {
        if !has_frame {
            return container(text("Preparing preview…").size(theme::LABEL).style(|t| text::Style { color: Some(theme::muted(t)) }))
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into();
        }
        let s = (bounds.width / vw).min(bounds.height / vh);
        let (w, h) = ((vw * s).max(1.0), (vh * s).max(1.0));
        container(shader(video::VideoProgram::new(frame.clone())).width(Length::Fixed(w)).height(Length::Fixed(h)))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    });
    let pane = container(video).width(Length::Fill).height(height).style(theme::panel);
    mouse_area(pane).on_double_click(Message::ToggleFullscreen).into()
}

/// A label-over-value stat group for the export bar.
fn stat<'a>(label: &'a str, value: String) -> Element<'a, Message> {
    column![
        text(label).size(theme::SMALL).style(|t| text::Style { color: Some(theme::muted(t)) }),
        text(value).size(theme::LABEL).font(Font::MONOSPACE),
    ]
    .spacing(2.0)
    .into()
}

/// A `label   value` row for the debug panel: a muted fixed-width label and a monospace value.
fn dbg_row<'a>(label: &'a str, value: String) -> Element<'a, Message> {
    row![
        text(label).size(theme::SMALL).width(Length::Fixed(130.0)).style(|t| text::Style { color: Some(theme::muted(t)) }),
        text(value).size(theme::SMALL).font(Font::MONOSPACE),
    ]
    .spacing(theme::SM)
    .align_y(iced::Alignment::Center)
    .into()
}

/// A tag pill.
fn chip<'a>(label: String) -> Element<'a, Message> {
    container(text(label).size(theme::SMALL)).padding([2.0, 8.0]).style(theme::chip).into()
}

/// A tag pill with a ✕ remove button.
fn removable_chip<'a>(label: String, on_remove: Message) -> Element<'a, Message> {
    let remove = with_tip(
        button(text("✕").size(theme::SMALL)).style(theme::btn_ghost).padding(0.0).on_press(on_remove).into(),
        "Remove tag".to_string(),
    );
    container(row![text(label).size(theme::SMALL), remove].spacing(theme::XS).align_y(iced::Alignment::Center))
        .padding([2.0, 8.0])
        .style(theme::chip)
        .into()
}

/// Wrap a widget in a tooltip with a styled bubble.
fn with_tip<'a>(content: Element<'a, Message>, label: String) -> Element<'a, Message> {
    tooltip(
        content,
        container(text(label).size(theme::SMALL)).padding([4.0, 8.0]).style(theme::card),
        tooltip::Position::Top,
    )
    .into()
}

/// A centered, muted placeholder filling the editor pane.
fn empty_state<'a>(msg: &'a str) -> Element<'a, Message> {
    container(text(msg).size(theme::HEADING).style(|t| text::Style { color: Some(theme::muted(t)) }))
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

fn num_field<'a>(label: &'a str, value: i64, on_input: impl Fn(String) -> Message + 'a) -> Element<'a, Message> {
    column![
        text(label).size(theme::SMALL).style(|t| text::Style { color: Some(theme::muted(t)) }),
        text_input("", &value.to_string()).on_input(on_input).font(Font::MONOSPACE).style(theme::input).width(Length::Fixed(110.0)),
    ]
    .spacing(theme::XS)
    .into()
}

fn kb_row<'a>(label: &'a str, value: &'a str, field: KbField) -> Element<'a, Message> {
    row![
        text(label).size(theme::LABEL).width(Length::Fixed(150.0)),
        text_input("unbound", value).on_input(move |s| Message::SetKeybind(field, s)).style(theme::input).width(Length::Fixed(150.0)),
    ]
    .spacing(theme::SM)
    .align_y(iced::Alignment::Center)
    .into()
}

fn section<'a>(title: &'a str, content: Element<'a, Message>) -> Element<'a, Message> {
    container(column![text(title).size(theme::HEADING).font(theme::FONT_SEMIBOLD), content].spacing(theme::SM))
        .padding(theme::LG)
        .style(theme::card)
        .into()
}

/// A modal dialog: a `dialog`-styled card centered over a dimming scrim. Clicking the backdrop (or
/// pressing Escape, wired in the subscription) sends `on_dismiss`.
fn modal<'a>(content: iced::widget::Column<'a, Message>, on_dismiss: Message) -> Element<'a, Message> {
    let card = container(content).padding(theme::XL).max_width(520).style(theme::dialog);
    opaque(mouse_area(center(opaque(card)).style(theme::scrim)).on_press(on_dismiss)).into()
}
