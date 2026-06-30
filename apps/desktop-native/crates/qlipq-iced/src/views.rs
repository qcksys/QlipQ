// View builders for `App`, include!d into main.rs (shares its imports).

impl App {
    fn top_bar(&self) -> Element<'_, Message> {
        let pending = self
            .items
            .iter()
            .filter(|i| i.status != QueueStatus::Done && !item_dismissed(i))
            .count();
        row![
            text("QlipQ").size(18),
            button(text("Powered by FFmpeg").size(12)).on_press(Message::OpenFfmpeg),
            Space::new().width(Length::Fill),
            button(text(format!("Queue ({pending})"))).on_press(Message::ShowQueue),
            button("Settings").on_press(Message::ShowSettings),
            button("GitHub").on_press(Message::OpenRepo),
        ]
        .spacing(8)
        .padding(10)
        .into()
    }

    fn queue_sidebar(&self) -> Element<'_, Message> {
        let mut all_tags: Vec<String> = self.items.iter().flat_map(|i| i.tags.clone().unwrap_or_default()).collect();
        all_tags.sort();
        all_tags.dedup();

        let visible: Vec<&QueueItem> = self
            .items
            .iter()
            .filter(|i| match &self.tag_filter {
                Some(f) if all_tags.contains(f) => i.tags.as_ref().map(|t| t.contains(f)).unwrap_or(false),
                _ => !item_dismissed(i),
            })
            .collect();

        let mut col = column![].spacing(8).padding(10);

        if !self.config.watched_folders.is_empty() {
            col = col.push(button("Rescan all folders").on_press(Message::RescanAll));
        }

        if !all_tags.is_empty() {
            let mut filters = row![button("All").on_press(Message::SetTagFilter(None))].spacing(6);
            for t in &all_tags {
                filters = filters.push(button(text(t.clone())).on_press(Message::SetTagFilter(Some(t.clone()))));
            }
            col = col.push(filters);
        }

        if visible.is_empty() {
            col = col.push(text("Queue is empty. Add a watched folder to populate it.").size(13));
        }

        let mut list = column![].spacing(8);
        for item in visible {
            list = list.push(self.queue_card(item));
        }
        col = col.push(scrollable(list).height(Length::Fill));

        if self.items.is_empty() && self.config.watched_folders.is_empty() {
            col = col.push(button("Add a watched folder →").on_press(Message::ShowSettings));
        }

        container(col).into()
    }

    fn queue_card(&self, item: &QueueItem) -> Element<'_, Message> {
        let selected = self.selected_id.as_deref() == Some(&item.id);
        let title = format!("{}{}", if selected { "▶ " } else { "" }, item.file_name);
        let open = button(
            column![text(title).size(14), text(meta_line(item)).size(11), text(status_label(item.status)).size(11)]
                .spacing(2),
        )
        .width(Length::Fill)
        .on_press(Message::SelectItem(item.id.clone()));

        let mut tags_row = row![].spacing(4);
        for t in item.tags.clone().unwrap_or_default() {
            tags_row = tags_row.push(text(format!("#{t}")).size(11));
        }

        let actions = row![
            button(text("Rename").size(12)).on_press(Message::RenameOpen(item.id.clone())),
            button(text(if item_dismissed(item) { "Restore" } else { "Dismiss" }).size(12)).on_press(Message::Dismiss(item.id.clone())),
            button(text("Delete").size(12)).on_press(Message::RequestDelete(item.id.clone())),
        ]
        .spacing(8);

        container(column![open, tags_row, actions].spacing(4)).padding(8).into()
    }

    fn editor_view(&self) -> Element<'_, Message> {
        let Some(ed) = &self.editor else {
            return container(text("Select a clip from the queue to start editing."))
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into();
        };
        if let Some(err) = &ed.load_error {
            return container(column![text("Could not read this clip."), text(err.clone()).size(12)].spacing(8))
                .padding(24)
                .into();
        }
        let Some(media) = &ed.media else {
            return container(text("Reading clip…")).center_x(Length::Fill).center_y(Length::Fill).into();
        };
        let Some(item) = self.items.iter().find(|i| i.id == ed.item_id) else {
            return container(text("…")).into();
        };

        // Preview frame.
        let preview: Element<Message> = match &ed.frame {
            Some(h) => image(h.clone()).width(Length::Fill).height(Length::Fixed(360.0)).into(),
            None => container(text("Preparing preview…")).center_x(Length::Fill).height(Length::Fixed(360.0)).into(),
        };

        // Transport.
        let transport = row![
            button("−60s").on_press(Message::Skip(-60.0)),
            button("−5s").on_press(Message::Skip(-5.0)),
            button("−1s").on_press(Message::Skip(-1.0)),
            button(if ed.playing { "Pause" } else { "Play" }).on_press(Message::TogglePlay),
            button("+1s").on_press(Message::Skip(1.0)),
            button("+5s").on_press(Message::Skip(5.0)),
            button("+60s").on_press(Message::Skip(60.0)),
        ]
        .spacing(4);

        // Timeline.
        let dur = media.duration_sec.max(0.001);
        let scrub = slider(0.0..=dur, ed.current_time.min(dur), Message::Seek).step(0.05);
        let timeline = column![
            scrub,
            row![
                text(format!("In {:.2}", ed.trim_start)).size(12),
                button("Set in at playhead").on_press(Message::SetIn),
                text(datetimes::format_duration(ed.trim_end - ed.trim_start)),
                button("Set out at playhead").on_press(Message::SetOut),
                text(format!("Out {:.2}", ed.trim_end)).size(12),
            ]
            .spacing(8),
        ]
        .spacing(8);

        // Crop + audio.
        let crop = self.crop_section(ed, media);
        let audio = self.audio_section(ed);
        let panels = row![container(crop).width(Length::Fill), container(audio).width(Length::Fill)].spacing(12);

        // Quality override.
        let override_section = self.override_section(item);

        // Tags.
        let tags = self.editor_tags(item);

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
        let summary = format!(
            "{} output · {} · {}{}",
            datetimes::format_duration(qlipq_core::edit_spec::effective_duration(&spec, media)),
            dims,
            if estimate.approximate { "≈" } else { "" },
            format_bytes(estimate.bytes),
        );

        let mut export_bar = row![text(summary).size(13)].spacing(8);
        if let Some(err) = &validation {
            export_bar = export_bar.push(text(err.clone()).size(13));
        }
        if ed.exporting {
            export_bar = export_bar.push(container(progress_bar(0.0..=1.0, ed.progress_display)).width(Length::Fixed(160.0)));
        }
        export_bar = export_bar.push(Space::new().width(Length::Fill));
        if item.export_path.is_some() && !ed.exporting {
            export_bar = export_bar.push(button("Show file").on_press(Message::ShowExported));
        }
        let can_export = validation.is_none() && !ed.exporting && !self.config.output_folder.is_empty();
        let export_btn = button(text(if ed.exporting {
            format!("Exporting {}%", (ed.progress_display * 100.0) as i32)
        } else {
            "Export clip".to_string()
        }));
        export_bar = export_bar.push(if can_export { export_btn.on_press(Message::Export) } else { export_btn });

        scrollable(
            column![preview, transport, timeline, panels, override_section, tags, export_bar]
                .spacing(16)
                .padding(16),
        )
        .into()
    }

    fn crop_section<'a>(&self, ed: &'a Editor, media: &MediaInfo) -> Element<'a, Message> {
        let mut col = column![
            text("Crop"),
            checkbox(ed.crop_enabled)
                .label(format!("Enable crop ({}×{} source)", media.width, media.height))
                .on_toggle(Message::ToggleCrop),
        ]
        .spacing(8);
        if ed.crop_enabled {
            col = col.push(
                row![
                    num_field("X", ed.crop.x, |s| Message::CropEdited(0, s)),
                    num_field("Y", ed.crop.y, |s| Message::CropEdited(1, s)),
                    num_field("W", ed.crop.width, |s| Message::CropEdited(2, s)),
                    num_field("H", ed.crop.height, |s| Message::CropEdited(3, s)),
                ]
                .spacing(8),
            );
        }
        container(col).padding(12).into()
    }

    fn audio_section<'a>(&self, ed: &'a Editor) -> Element<'a, Message> {
        let mut col = column![text("Audio tracks")].spacing(8);
        if ed.audio.is_empty() {
            col = col.push(text("No audio tracks in this clip.").size(12));
        }
        for r in &ed.audio {
            let idx = r.index;
            col = col.push(
                column![
                    row![
                        checkbox(r.enabled).label(r.label.clone()).on_toggle(move |on| Message::AudioToggle(idx, on)),
                        text(r.detail.clone()).size(11),
                    ]
                    .spacing(8),
                    row![
                        container(slider(0.0..=2.0, r.volume, move |v| Message::AudioVolume(idx, v)).step(0.05))
                            .width(Length::Fixed(180.0)),
                        text(format!("{}%", (r.volume * 100.0) as i32)).size(11),
                    ]
                    .spacing(8),
                ]
                .spacing(4),
            );
        }
        container(col).padding(12).into()
    }

    fn override_section(&self, item: &QueueItem) -> Element<'_, Message> {
        let enabled = item.output_override.is_some();
        let mut col = column![checkbox(enabled).label("Override quality for this clip").on_toggle(Message::ToggleOverride)].spacing(8);
        if enabled {
            let out = self.effective_output(item);
            let mut fields = row![pick_list(QmChoice::ALL.to_vec(), Some(QmChoice::from_core(out.quality_mode)), Message::OverrideQm)].spacing(8);
            match out.quality_mode {
                QualityMode::Preset => {
                    fields = fields.push(pick_list(QpChoice::ALL.to_vec(), Some(QpChoice::from_core(out.quality_preset)), Message::OverrideQp));
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
        container(col).padding(12).into()
    }

    fn editor_tags(&self, item: &QueueItem) -> Element<'_, Message> {
        let mut tags_row = row![].spacing(6);
        for t in item.tags.clone().unwrap_or_default() {
            let tag = t.clone();
            tags_row = tags_row.push(
                row![text(t).size(12), button(text("✕").size(11)).on_press(Message::RemoveTag(tag))].spacing(2),
            );
        }
        let input = text_input("Add tag…", &self.new_tag).on_input(Message::NewTagChanged).on_submit(Message::AddTag).width(Length::Fixed(160.0));
        container(column![text("Tags"), row![tags_row, input].spacing(8)].spacing(8)).padding(12).into()
    }

    fn settings_view(&self) -> Element<'_, Message> {
        let out = &self.config.output;

        // Watched folders.
        let mut folders = column![text("Watched folders")].spacing(8);
        for f in &self.config.watched_folders {
            folders = folders.push(
                row![
                    text(f.clone()).width(Length::Fill),
                    button(text("Reprocess").size(12)).on_press(Message::Reprocess(f.clone())),
                    button(text("Remove").size(12)).on_press(Message::RemoveFolder(f.clone())),
                ]
                .spacing(8),
            );
        }
        let mut add_row = row![button("Add folder…").on_press(Message::PickFolder(PickPurpose::WatchedFolder))].spacing(8);
        if let Some(obs) = &self.presets.obs {
            if !self.config.watched_folders.contains(obs) {
                add_row = add_row.push(button(text(format!("+ OBS ({obs})")).size(12)).on_press(Message::AddPreset(obs.clone())));
            }
        }
        if let Some(nv) = &self.presets.nvidia_share {
            if !self.config.watched_folders.contains(nv) {
                add_row = add_row.push(button(text(format!("+ NVIDIA Share ({nv})")).size(12)).on_press(Message::AddPreset(nv.clone())));
            }
        }
        folders = folders.push(add_row);

        // Output folder.
        let output_folder = row![
            text_input("Where exported clips are saved", &self.config.output_folder).on_input(Message::OutputFolderChanged).width(Length::Fill),
            button("Browse…").on_press(Message::PickFolder(PickPurpose::OutputFolder)),
        ]
        .spacing(8);

        // Output defaults.
        let mut quality = row![pick_list(QmChoice::ALL.to_vec(), Some(QmChoice::from_core(out.quality_mode)), Message::SetQm)].spacing(8);
        match out.quality_mode {
            QualityMode::Preset => quality = quality.push(pick_list(QpChoice::ALL.to_vec(), Some(QpChoice::from_core(out.quality_preset)), Message::SetQp)),
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
            pick_list(encoder_options, Some(out.encoder_preset.clone()), Message::SetEncoder),
            pick_list(CodecChoice::ALL.to_vec(), Some(CodecChoice::from_core(out.video_codec)), Message::SetCodec),
            pick_list(ContainerChoice::ALL.to_vec(), Some(ContainerChoice::from_core(out.container)), Message::SetContainer),
        ]
        .spacing(8);
        let rate_row = row![
            pick_list(FpsChoice::ALL.to_vec(), Some(FpsChoice::from_core(out.fps)), Message::SetFps),
            pick_list(ResChoice::ALL.to_vec(), Some(ResChoice::from_core(out.max_height)), Message::SetRes),
            pick_list(AudioKbpsChoice::ALL.to_vec(), Some(AudioKbpsChoice::from_core(out.audio_bitrate_kbps)), Message::SetAudioKbps),
        ]
        .spacing(8);

        // FFmpeg.
        let ffmpeg_row = row![
            text_input("ffmpeg", &self.config.ffmpeg_path).on_input(Message::FfmpegPathChanged).width(Length::Fill),
            button("Test").on_press(Message::TestFfmpeg),
        ]
        .spacing(8);
        let ffprobe_row = row![
            text_input("ffprobe", &self.config.ffprobe_path).on_input(Message::FfprobePathChanged).width(Length::Fill),
            button("Test").on_press(Message::TestFfprobe),
        ]
        .spacing(8);
        let ffmpeg_status = test_text(&self.ffmpeg_test);
        let ffprobe_status = test_text(&self.ffprobe_test);

        // After export.
        let ae = &self.config.after_export;
        let mut after = column![pick_list(AfterChoice::ALL.to_vec(), Some(AfterChoice::from_core(ae.action)), Message::SetAfter)].spacing(8);
        if ae.action == AfterExportAction::Move {
            after = after.push(
                row![
                    text_input("Destination folder", &ae.move_folder).on_input(Message::MoveFolderChanged).width(Length::Fill),
                    button("Browse…").on_press(Message::PickFolder(PickPurpose::MoveFolder)),
                ]
                .spacing(8),
            );
        }
        if ae.action == AfterExportAction::Rename {
            after = after.push(
                row![
                    text_input("Prefix", &ae.rename_prefix).on_input(Message::RenamePrefixChanged),
                    text_input("Suffix", &ae.rename_suffix).on_input(Message::RenameSuffixChanged),
                ]
                .spacing(8),
            );
        }

        let body = column![
            section("Watched folders", folders.into()),
            section("Output folder", output_folder.into()),
            section("Output defaults", column![quality, encode_row, rate_row].spacing(8).into()),
            section("Naming template", column![
                text_input("{date}_{source}_{name}", &self.config.naming_template).on_input(Message::NamingChanged),
                text("Tokens: {date} {time} {datetime} {source} {name} {index}").size(11),
            ].spacing(6).into()),
            section("FFmpeg", column![ffmpeg_row, ffmpeg_status, ffprobe_row, ffprobe_status].spacing(8).into()),
            section("After export", after.into()),
            button("Open config file").on_press(Message::OpenConfigFile),
        ]
        .spacing(16)
        .padding(20);

        scrollable(container(body).max_width(760.0).center_x(Length::Fill)).into()
    }

    // ---- Modals ----

    fn rename_modal<'a>(&'a self, r: &'a RenameState) -> Element<'a, Message> {
        modal(column![
            text("Rename recording").size(18),
            text_input("name", &r.value).on_input(Message::RenameValue).on_submit(Message::RenameConfirm),
            row![
                button("Use template").on_press(Message::RenameTemplate),
                Space::new().width(Length::Fill),
                button("Cancel").on_press(Message::RenameCancel),
                button("Rename").on_press(Message::RenameConfirm),
            ]
            .spacing(8),
        ]
        .spacing(12))
    }

    fn delete_modal(&self, id: &str) -> Element<'_, Message> {
        let name = self.items.iter().find(|i| i.id == id).map(|i| i.file_name.clone()).unwrap_or_default();
        modal(column![
            text("Delete this file from disk?").size(18),
            text(format!("{name} will be permanently deleted. This can't be undone.")),
            row![
                Space::new().width(Length::Fill),
                button("Cancel").on_press(Message::DeleteCancel),
                button("Delete").on_press(Message::DeleteConfirm),
            ]
            .spacing(8),
        ]
        .spacing(12))
    }

    fn overwrite_modal(&self, target: &str) -> Element<'_, Message> {
        modal(column![
            text("Overwrite existing file?").size(18),
            text(format!("A file already exists at {target}. Exporting will replace it.")),
            row![
                button("Cancel").on_press(Message::Overwrite(2)),
                Space::new().width(Length::Fill),
                button("Append timestamp").on_press(Message::Overwrite(1)),
                button("Overwrite").on_press(Message::Overwrite(0)),
            ]
            .spacing(8),
        ]
        .spacing(12))
    }

    fn after_modal(&self) -> Element<'_, Message> {
        modal(column![
            text("Export complete").size(18),
            text("What should happen to the original recording?"),
            row![
                button("Keep").on_press(Message::AfterChoice(AfterExportAction::Nothing)),
                button("Rename").on_press(Message::AfterChoice(AfterExportAction::Rename)),
                button("Move…").on_press(Message::AfterChoice(AfterExportAction::Move)),
                button("Delete").on_press(Message::AfterChoice(AfterExportAction::Delete)),
            ]
            .spacing(8),
        ]
        .spacing(12))
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

fn num_field<'a>(label: &'a str, value: i64, on_input: impl Fn(String) -> Message + 'a) -> Element<'a, Message> {
    column![text(label).size(11), text_input("", &value.to_string()).on_input(on_input).width(Length::Fixed(110.0))]
        .spacing(2)
        .into()
}

fn test_text(status: &Option<(bool, String)>) -> Element<'_, Message> {
    match status {
        Some((ok, msg)) => text(format!("{} {}", if *ok { "✓" } else { "✗" }, msg)).size(11).into(),
        None => Space::new().into(),
    }
}

fn section<'a>(title: &'a str, content: Element<'a, Message>) -> Element<'a, Message> {
    container(column![text(title).size(15), content].spacing(8)).padding(12).into()
}

fn modal(content: iced::widget::Column<'_, Message>) -> Element<'_, Message> {
    container(container(content).padding(20).max_width(520))
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .padding(40)
        .into()
}
