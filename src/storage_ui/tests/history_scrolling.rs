use super::*;
use i_slint_backend_testing::ElementHandle;
use slint::{
    LogicalPosition, LogicalSize,
    platform::{PointerEventButton, WindowEvent},
};
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Debug)]
struct PageObservation {
    older: bool,
    anchor: String,
    offset: f32,
    anchor_y: f32,
    rows: usize,
    distance_to_top: f32,
    visible_height: f32,
    anchor_index: usize,
}

fn viewport(ui: &AppWindow) -> ElementHandle {
    ElementHandle::find_by_element_id(ui, "MessageTimeline::timeline")
        .next()
        .expect("the real Slint timeline must be rendered")
}

fn identifies(bubble: &ElementHandle, message: &StoredMessage) -> bool {
    let body = message.body.clone();
    let exact_time = message.exact_time.clone();
    bubble
        .query_descendants()
        .match_predicate(move |element| {
            element
                .accessible_label()
                .is_some_and(|label| label == body || label == exact_time)
        })
        .find_first()
        .is_some()
}

fn message_bubble(ui: &AppWindow, key: &str) -> ElementHandle {
    let messages = ui.get_stored_messages();
    let index = (0..messages.row_count())
        .find(|&index| messages.row_data(index).unwrap().key == key)
        .expect("the anchor must remain in the bounded window");
    let message = messages.row_data(index).unwrap();
    let bubbles = ElementHandle::find_by_element_id(ui, "MessageBubble::root").collect::<Vec<_>>();
    if let Some(bubble) = bubbles.iter().find(|bubble| identifies(bubble, &message)) {
        return bubble.clone();
    }
    // Only the bottom inset can remain on screen. Verify its next neighbour's
    // identity as well as intersection, so a wrong restored range cannot pass.
    if let Some(next) = messages.row_data(index + 1) {
        let top = viewport(ui).absolute_position().y;
        if let Some(pair) = bubbles.windows(2).find(|pair| {
            let y = pair[0].absolute_position().y;
            y < top && y + pair[0].size().height > top && identifies(&pair[1], &next)
        }) {
            return pair[0].clone();
        }
    }
    let rendered = ElementHandle::find_by_element_id(ui, "MessageBubble::body")
        .map(|element| {
            (
                element.id(),
                element.accessible_label(),
                element.absolute_position(),
            )
        })
        .collect::<Vec<_>>();
    panic!(
        "anchor bubble {:?} was not visible; body elements: {rendered:?}",
        message.body
    )
}

fn first_key(ui: &AppWindow) -> String {
    ui.get_stored_messages()
        .row_data(0)
        .unwrap()
        .key
        .to_string()
}

fn anchor_position(ui: &AppWindow) -> f32 {
    message_bubble(ui, &ui.get_timeline_anchor_key())
        .absolute_position()
        .y
        - viewport(ui).absolute_position().y
}

fn wheel(ui: &AppWindow, delta_y: f32) {
    let view = viewport(ui);
    let origin = view.absolute_position();
    let size = view.size();
    let position = LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    ui.window().dispatch_event(WindowEvent::PointerScrolled {
        position,
        delta_x: 0.0,
        delta_y,
    });
}

fn scrollbar_wheel(ui: &AppWindow, delta_y: f32) {
    let view = viewport(ui);
    let p = view.absolute_position();
    let size = view.size();
    let position = LogicalPosition::new(p.x + size.width - 4.0, p.y + size.height / 2.0);
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    ui.window().dispatch_event(WindowEvent::PointerScrolled {
        position,
        delta_x: 0.0,
        delta_y,
    });
}

fn drag_scrollbar_to_top(ui: &AppWindow) {
    // A real thumb drag emits ScrollView.scrolled synchronously, unlike a
    // scrollbar wheel event, and leaves no wheel animation after the input.
    let view = viewport(ui);
    let origin = view.absolute_position();
    let size = view.size();
    let thumb = view
        .query_descendants()
        .match_id("ScrollBar::thumb")
        .find_all()
        .into_iter()
        .find(|thumb| thumb.size().height > thumb.size().width && thumb.size().height > 0.0)
        .expect("the loading timeline must have a vertical scrollbar thumb");
    let p = thumb.absolute_position();
    let thumb_size = thumb.size();
    let start = LogicalPosition::new(p.x + thumb_size.width / 2.0, p.y + thumb_size.height / 2.0);
    let target = LogicalPosition::new(start.x, origin.y - size.height);
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: start });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: start,
        button: PointerEventButton::Left,
    });
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: target });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: target,
        button: PointerEventButton::Left,
    });
}

fn assert_anchor(ui: &AppWindow, expected: &PageObservation) {
    eprintln!(
        "assert-anchor expected={expected:?} actual-key={} offset={} viewport={:?} history={:?}",
        ui.get_timeline_anchor_key(),
        ui.get_timeline_anchor_offset(),
        viewport(ui).absolute_position(),
        ElementHandle::find_by_element_id(ui, "MessageTimeline::history")
            .next()
            .map(|e| (e.absolute_position(), e.size()))
    );
    assert_eq!(ui.get_timeline_anchor_key(), expected.anchor);
    assert!(
        (ui.get_timeline_anchor_offset() - expected.offset).abs() <= 1.5,
        "stored offset changed: {:?} -> {}",
        expected,
        ui.get_timeline_anchor_offset()
    );
    assert!(
        (anchor_position(ui) - expected.anchor_y).abs() <= 1.5,
        "visible anchor jumped: {:?} -> {}",
        expected,
        anchor_position(ui)
    );
}

fn snapshot(ui: &AppWindow, native: bool, name: &str) {
    if !native {
        return;
    }
    let image = ui.window().take_snapshot().unwrap();
    let mut bytes = format!("P6\n{} {}\n255\n", image.width(), image.height()).into_bytes();
    for pixel in image.as_slice() {
        bytes.extend_from_slice(&[pixel.r, pixel.g, pixel.b]);
    }
    std::fs::create_dir_all("target/history-prefetch").unwrap();
    std::fs::write(format!("target/history-prefetch/{name}.ppm"), bytes).unwrap();
}

struct Scenario {
    phase: u8,
    entered: Instant,
    started: Instant,
    last_model_change: Instant,
    model_stamp: Option<(i32, usize, String, bool, bool, u32, u32)>,
    requests: Rc<RefCell<Vec<PageObservation>>>,
    chat: i64,
    other: i64,
    oldest: String,
    latest_first: String,
    saved: Option<PageObservation>,
    quiet_count: usize,
    thumb_start: Option<LogicalPosition>,
    inject_while_loading: Rc<Cell<bool>>,
    native: bool,
    loading_viewport: Option<ElementHandle>,
}

impl Scenario {
    fn next(&mut self) {
        eprintln!("history phase {} -> {}", self.phase, self.phase + 1);
        self.phase += 1;
        self.entered = Instant::now();
    }
    fn settled(&self) -> bool {
        self.entered.elapsed() > Duration::from_millis(160)
            && self.last_model_change.elapsed() > Duration::from_millis(160)
    }

    fn tick(&mut self, ui: &AppWindow) -> bool {
        if !self.native {
            // Materialize model/repeater changes as a native drawing pass would.
            i_slint_backend_testing::mock_elapsed_time(Duration::ZERO);
        }
        assert!(
            self.started.elapsed() < Duration::from_secs(180),
            "history scenario timed out in phase {}: requests={:?}, anchor={} offset={} rows={} loading={}",
            self.phase,
            self.requests.borrow(),
            ui.get_timeline_anchor_key(),
            ui.get_timeline_anchor_offset(),
            ui.get_stored_messages().row_count(),
            ui.get_timeline_loading()
        );
        let model = ui.get_stored_messages();
        let count = model.row_count();
        let size = ui.window().size();
        let stamp = (
            ui.get_timeline_revision(),
            count,
            model
                .row_data(0)
                .map(|row| row.key.to_string())
                .unwrap_or_default(),
            ui.get_timeline_loading(),
            ui.get_conversation_loading(),
            size.width,
            size.height,
        );
        if self.model_stamp.as_ref() != Some(&stamp) {
            self.model_stamp = Some(stamp);
            self.last_model_change = Instant::now();
        }
        if count > 0 && !ui.get_selected_conversation().is_empty() {
            // The lightweight testing backend does not draw frames itself.
            // Evaluate the real layout, then allow its change handlers and the
            // existing 8ms restoration pass to run before checking geometry.
            let view = viewport(ui);
            let _ = (view.absolute_position(), view.size());
            if let Some(history) =
                ElementHandle::find_by_element_id(ui, "MessageTimeline::history").next()
            {
                let _ = (history.absolute_position(), history.size());
            }
        }
        assert!(count <= 120);
        if ui.get_storage_ready() {
            assert!(ui.get_conversation_error().is_empty());
        }
        match self.phase {
            0 if ui.get_storage_ready() => {
                ui.invoke_select_stored_conversation(self.chat.to_string().into());
                self.next();
            }
            1 if !ui.get_conversation_loading()
                && ui.get_stored_messages().row_count() == 60
                && self.settled() =>
            {
                assert!(ui.get_timeline_following());
                assert!(
                    self.requests.borrow().is_empty(),
                    "opening must not paginate"
                );
                self.latest_first = first_key(ui);
                snapshot(ui, self.native, "01-initial-1080x680");
                self.next();
            }
            2 if self.requests.borrow().is_empty() => wheel(ui, 37.0),
            2 if !ui.get_timeline_loading() && self.settled() => {
                let request = self.requests.borrow()[0].clone();
                assert!(request.older);
                assert_eq!(request.rows, 60);
                assert!(
                    request.anchor_index >= 3,
                    "prefetch should precede the first few rows: {request:?}"
                );
                assert!(
                    request.distance_to_top > 96.0,
                    "prefetch still waits for the old boundary: {request:?}"
                );
                assert!(
                    request.distance_to_top <= (2.0 * request.visible_height).max(640.0) + 80.0
                );
                assert_ne!(first_key(ui), self.latest_first);
                assert_eq!(ui.get_stored_messages().row_count(), 100);
                assert_anchor(ui, &request);
                self.saved = Some(request);
                self.quiet_count = self.requests.borrow().len();
                self.next();
            }
            3 if self.settled() => {
                assert_eq!(
                    self.requests.borrow().len(),
                    self.quiet_count,
                    "restoration must not recursively request more pages"
                );
                self.next();
            }
            4 if self.requests.borrow().len() == self.quiet_count => wheel(ui, 173.0),
            4 if !ui.get_timeline_loading() && self.settled() => {
                let request = self.requests.borrow().last().unwrap().clone();
                assert!(request.older);
                assert_eq!(ui.get_stored_messages().row_count(), 120);
                assert!(
                    ui.get_timeline_has_newer(),
                    "trimming newer rows must mark the gap"
                );
                assert_anchor(ui, &request);
                self.saved = Some(request);
                self.quiet_count = self.requests.borrow().len();
                snapshot(ui, self.native, "02-bounded-older-window");
                self.next();
            }
            5 if self.settled() => {
                assert_eq!(self.requests.borrow().len(), self.quiet_count);
                ui.invoke_select_stored_conversation(self.other.to_string().into());
                self.next();
            }
            6 if !ui.get_conversation_loading()
                && ui.get_selected_conversation() == self.other.to_string()
                && ui.get_stored_messages().row_count() == 12
                && self.settled() =>
            {
                assert!(ui.get_timeline_following());
                assert_eq!(self.requests.borrow().len(), self.quiet_count);
                ui.invoke_select_stored_conversation(self.chat.to_string().into());
                self.next();
            }
            7 if !ui.get_conversation_loading()
                && ui.get_selected_conversation() == self.chat.to_string()
                && self.settled() =>
            {
                assert_anchor(ui, self.saved.as_ref().unwrap());
                assert_eq!(self.requests.borrow().len(), self.quiet_count);
                ui.window().set_size(LogicalSize::new(850.0, 560.0));
                self.next();
            }
            8 if self.settled() => {
                assert_anchor(ui, self.saved.as_ref().unwrap());
                assert_eq!(
                    self.requests.borrow().len(),
                    self.quiet_count,
                    "resize cannot authorize pagination"
                );
                snapshot(ui, self.native, "03-restored-850x560");
                ui.window().set_size(LogicalSize::new(1600.0, 900.0));
                self.next();
            }
            9 if self.settled() => {
                assert_anchor(ui, self.saved.as_ref().unwrap());
                assert_eq!(self.requests.borrow().len(), self.quiet_count);
                snapshot(ui, self.native, "04-restored-1600x900");
                if self.native {
                    ui.window().set_maximized(true);
                    self.phase = 30;
                    self.entered = Instant::now();
                } else {
                    ui.window().set_size(LogicalSize::new(1080.0, 680.0));
                    self.next();
                }
            }
            30 if self.settled() => {
                assert_anchor(ui, self.saved.as_ref().unwrap());
                assert_eq!(self.requests.borrow().len(), self.quiet_count);
                snapshot(ui, self.native, "04b-restored-maximized");
                ui.window().set_maximized(false);
                ui.window().set_size(LogicalSize::new(1080.0, 680.0));
                self.phase = 10;
                self.entered = Instant::now();
            }
            10 if self.settled() => {
                // The native scrollbar is exercised directly, without assigning content-y.
                let view = viewport(ui);
                let thumb = view
                    .query_descendants()
                    .match_id("ScrollBar::thumb")
                    .find_all()
                    .into_iter()
                    .find(|thumb| {
                        thumb.size().height > thumb.size().width && thumb.size().height > 0.0
                    })
                    .expect("the timeline must expose its vertical scrollbar thumb");
                let p = thumb.absolute_position();
                let size = thumb.size();
                let start = LogicalPosition::new(p.x + size.width / 2.0, p.y + size.height / 2.0);
                ui.window()
                    .dispatch_event(WindowEvent::PointerMoved { position: start });
                ui.window().dispatch_event(WindowEvent::PointerPressed {
                    position: start,
                    button: PointerEventButton::Left,
                });
                self.thumb_start = Some(start);
                self.next();
            }
            11 => {
                let start = self.thumb_start.unwrap();
                let target = LogicalPosition::new(start.x, start.y + 36.0);
                ui.window()
                    .dispatch_event(WindowEvent::PointerMoved { position: target });
                ui.window().dispatch_event(WindowEvent::PointerReleased {
                    position: target,
                    button: PointerEventButton::Left,
                });
                self.next();
            }
            12 if self.settled() => {
                assert_ne!(
                    ui.get_timeline_anchor_key(),
                    self.saved.as_ref().unwrap().anchor,
                    "dragging the thumb should move the reading position"
                );
                self.quiet_count = self.requests.borrow().len();
                self.next();
            }
            13 if self.requests.borrow().len() == self.quiet_count => wheel(ui, -211.0),
            13 if !ui.get_timeline_loading() && self.settled() => {
                assert!(
                    !self.requests.borrow().last().unwrap().older,
                    "downward input must page newer history"
                );
                self.quiet_count = self.requests.borrow().len();
                ui.invoke_jump_to_present();
                self.next();
            }
            14 if !ui.get_timeline_loading() && self.settled() => {
                assert!(ui.get_timeline_following());
                assert!(!ui.get_timeline_has_newer());
                assert_eq!(ui.get_stored_messages().row_count(), 60);
                assert_eq!(first_key(ui), self.latest_first);
                ui.invoke_open_quote(self.oldest.clone().into());
                self.next();
            }
            15 if !ui.get_timeline_loading()
                && ui.get_timeline_anchor_key() == self.oldest
                && self.settled() =>
            {
                assert!(!ui.get_timeline_following());
                assert!(ui.get_timeline_has_newer());
                assert_eq!(first_key(ui), self.oldest);
                assert_eq!(
                    self.requests.borrow().len(),
                    self.quiet_count,
                    "quote restoration cannot trigger opposite-direction pagination"
                );
                snapshot(ui, self.native, "05-original-outside-recent-window");
                ui.invoke_jump_to_present();
                ui.window().set_size(LogicalSize::new(1600.0, 1200.0));
                self.next();
            }
            16 if !ui.get_timeline_loading() && self.settled() => {
                self.quiet_count = self.requests.borrow().len();
                // A single wheel gesture must not mint further demand as it animates.
                wheel(ui, 499.0);
                self.next();
            }
            17 if self.entered.elapsed() > Duration::from_millis(700)
                && self.settled()
                && !ui.get_timeline_loading() =>
            {
                assert!(
                    self.requests.borrow().len() <= self.quiet_count + 1,
                    "one wheel input caused repeated pages: {:?}",
                    self.requests.borrow()
                );
                snapshot(ui, self.native, "06-single-wheel-no-recursive-load");
                ui.invoke_jump_to_present();
                // Keep a prepended page within two viewports so retained input
                // is still eligible after restoring the old topmost message.
                ui.window().set_size(LogicalSize::new(1600.0, 1400.0));
                self.next();
            }
            18 if !ui.get_timeline_loading() && self.settled() => {
                self.quiet_count = self.requests.borrow().len();
                wheel(ui, 3.0);
                self.next();
            }
            19 if self.entered.elapsed() > Duration::from_millis(300) => {
                assert!(
                    ui.get_timeline_following(),
                    "tiny upward input remains inside the bottom tolerance"
                );
                assert_eq!(
                    self.requests.borrow().len(),
                    self.quiet_count,
                    "a tiny movement at the present must not wait forever for a non-following anchor"
                );
                self.inject_while_loading.set(true);
                self.loading_viewport = Some(viewport(ui));
                wheel(ui, 499.0);
                self.next();
            }
            20 if self.entered.elapsed() > Duration::from_millis(700)
                && self.settled()
                && self.requests.borrow().len() >= self.quiet_count + 2
                && !ui.get_timeline_loading() =>
            {
                assert!(
                    !self.inject_while_loading.get(),
                    "the loading-input probe must run"
                );
                assert_eq!(
                    self.requests.borrow().len(),
                    self.quiet_count + 2,
                    "one initial gesture plus one explicit loading-time gesture must each authorize one page"
                );
                assert!(
                    self.requests.borrow()[self.quiet_count..]
                        .iter()
                        .all(|page| page.older),
                    "restoration cannot reverse the retained loading-time direction"
                );
                assert_eq!(ui.get_stored_messages().row_count(), 120);
                snapshot(ui, self.native, "07-loading-input-retained-once");
                self.quiet_count = self.requests.borrow().len();
                self.next();
            }
            20 if self.entered.elapsed() > Duration::from_secs(5) => {
                panic!(
                    "retained input did not resume: same timeline={} loading={} rows={} anchor={} offset={} history={:?}",
                    self.loading_viewport.as_ref().unwrap().is_valid(),
                    ui.get_timeline_loading(),
                    ui.get_stored_messages().row_count(),
                    ui.get_timeline_anchor_key(),
                    ui.get_timeline_anchor_offset(),
                    ElementHandle::find_by_element_id(ui, "MessageTimeline::history")
                        .next()
                        .map(|h| (h.absolute_position(), h.size()))
                );
            }
            21 if !ui.get_timeline_loading() && self.settled() => {
                assert!(self.requests.borrow().len() <= self.quiet_count + 10);
                if ui.get_timeline_has_older() {
                    if self.requests.borrow().len() % 2 == 0 {
                        wheel(ui, 320.0);
                    } else {
                        scrollbar_wheel(ui, 320.0);
                    }
                } else {
                    assert_eq!(first_key(ui), self.oldest);
                    snapshot(ui, self.native, "08-oldest-history-after-fast-scroll");
                    self.next();
                }
            }
            22 if !ui.get_timeline_loading() && self.settled() => {
                assert!(self.requests.borrow().len() <= self.quiet_count + 20);
                if ui.get_timeline_has_newer() || !ui.get_timeline_following() {
                    wheel(ui, -640.0);
                } else {
                    let model = ui.get_stored_messages();
                    assert_eq!(
                        model.row_data(model.row_count() - 1).unwrap().body,
                        "History 359"
                    );
                    snapshot(ui, self.native, "09-latest-after-newer-pagination");
                    return true;
                }
            }
            _ => {}
        }
        false
    }
}

fn run_scenario(native: bool) {
    let root = TestDir::new();
    let key = ed25519_dalek::SigningKey::from_bytes(&[119; 32]);
    let public = key.verifying_key();
    let mut db = Database::open(
        &root.0,
        Owner {
            void_id: "41",
            public_key: public.as_bytes(),
        },
    )
    .unwrap();
    let chat = db.create_conversation("123", 100).unwrap();
    let other = db.create_conversation("456", 100).unwrap();
    for i in 0..360 {
        let body = match i % 11 {
            0 => format!(
                "History {i:03}: a longer line wraps naturally while resizing the native window and must preserve the same visible message.\nA second intentional line."
            ),
            _ => format!("History {i:03}"),
        };
        db.save_message(&Message {
            id: format!("history-{i:03}"),
            conversation_id: chat,
            direction: if i % 2 == 0 {
                Direction::Incoming
            } else {
                Direction::Outgoing
            },
            body,
            created_at: 1_790_000_000_000 + i * 60_000,
            delivery_state: if i % 2 == 0 {
                DeliveryState::Received
            } else {
                DeliveryState::Sent
            },
        })
        .unwrap();
    }
    for i in 0..12 {
        db.save_message(&Message {
            id: format!("other-{i:03}"),
            conversation_id: other,
            direction: Direction::Incoming,
            body: format!("Other conversation {i}"),
            created_at: 1_790_000_000_000 + i * 60_000,
            delivery_state: DeliveryState::Received,
        })
        .unwrap();
    }
    db.save_draft(chat, "Keep the isolated fixture draft")
        .unwrap();
    let oldest = db.timeline_before(chat, None, usize::MAX).unwrap()[0]
        .cursor
        .key();
    db.close().unwrap();
    if !native {
        i_slint_backend_testing::init_integration_test_with_system_time();
    }
    let ui = AppWindow::new().unwrap();
    ui.set_account_void_id("41".into());
    ui.set_setup_page(0);
    ui.window().set_size(LogicalSize::new(1080.0, 680.0));
    let path = root.0.clone();
    let worker = bind_fixture(&ui, move |_| (path.clone(), key.clone())).unwrap();
    let requests = Rc::new(RefCell::new(Vec::<PageObservation>::new()));
    let observed = requests.clone();
    let inject_while_loading = Rc::new(Cell::new(false));
    let inject = inject_while_loading.clone();
    let weak = ui.as_weak();
    let sender = worker.sender.clone();
    ui.on_page_timeline(move |older| {
        let ui = weak.upgrade().unwrap();
        let anchor = ui.get_timeline_anchor_key().to_string();
        let model = ui.get_stored_messages();
        let index = (0..model.row_count())
            .find(|&i| model.row_data(i).unwrap().key == anchor)
            .expect("pagination must capture a valid anchor BEFORE replacing the window");
        let view = viewport(&ui);
        let top = view.absolute_position().y;
        let history = ElementHandle::find_by_element_id(&ui, "MessageTimeline::history")
            .next()
            .expect("the timeline history layout must expose its real content position");
        let request = PageObservation {
            older,
            anchor: anchor.clone(),
            offset: ui.get_timeline_anchor_offset(),
            anchor_y: anchor_position(&ui),
            rows: model.row_count(),
            distance_to_top: (top - history.absolute_position().y).max(0.0),
            visible_height: view.size().height,
            anchor_index: index,
        };
        eprintln!(
            "request page {request:?} viewport={:?} history={:?}",
            view.absolute_position(),
            history.absolute_position()
        );
        observed.borrow_mut().push(request);
        request_timeline(&ui, &sender, |id, selection| {
            crate::storage_ui::Command::Page(id, selection, older)
        });
        if inject.replace(false) {
            assert!(ui.get_timeline_loading());
            drag_scrollbar_to_top(&ui);
            eprintln!(
                "loading-time thumb input anchor={} offset={}",
                ui.get_timeline_anchor_key(),
                ui.get_timeline_anchor_offset()
            );
        }
    });
    ui.show().unwrap();
    worker.opener()(&ui);
    let scenario = Rc::new(RefCell::new(Scenario {
        phase: 0,
        entered: Instant::now(),
        started: Instant::now(),
        last_model_change: Instant::now(),
        model_stamp: None,
        requests,
        chat,
        other,
        oldest,
        latest_first: String::new(),
        saved: None,
        quiet_count: 0,
        thumb_start: None,
        inject_while_loading,
        native,
        loading_viewport: None,
    }));
    let state = scenario.clone();
    let weak = ui.as_weak();
    let timer = Timer::default();
    let queued = Rc::new(Cell::new(false));
    timer.start(TimerMode::Repeated, Duration::from_millis(45), move || {
        if queued.replace(true) {
            return;
        }
        let (weak, state, queued) = (weak.clone(), state.clone(), queued.clone());
        // Drive frames outside TimerList activation: advancing the headless
        // renderer from inside another timer would recursively activate timers.
        slint::spawn_local(async move {
            let ui = weak.upgrade().unwrap();
            let completed = state.borrow_mut().tick(&ui);
            queued.set(false);
            if completed {
                ui.hide().unwrap();
                slint::quit_event_loop().unwrap();
            }
        })
        .unwrap();
    });
    slint::run_event_loop().unwrap();
    timer.stop();
    worker.finish();
    assert_eq!(scenario.borrow().phase, 22);
    let db = Database::open(
        &root.0,
        Owner {
            void_id: "41",
            public_key: public.as_bytes(),
        },
    )
    .unwrap();
    assert_eq!(
        db.timeline_before(chat, None, usize::MAX).unwrap().len(),
        360
    );
    assert_eq!(
        db.draft(chat).unwrap().as_deref(),
        Some("Keep the isolated fixture draft")
    );
}

#[test]
fn viewport_prefetch_preserves_real_worker_history_and_reading_positions() {
    if !in_test_process(
        "storage_ui::tests::history_scrolling::viewport_prefetch_preserves_real_worker_history_and_reading_positions",
    ) {
        return;
    }
    std::thread::spawn(|| run_scenario(false)).join().unwrap();
}

#[test]
#[ignore = "isolated native history rendering; no saved accounts, registration, or network"]
fn render_isolated_history_prefetch_at_desktop_sizes() {
    run_scenario(true);
}
