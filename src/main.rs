use egui_sdl2_gl::{
    egui, gl, sdl2, ShaderVersion, DpiScaling,
    painter::Painter
};

use egui::{
    RawInput, FullOutput
};

use openvr_sys2::{
    VROverlayFlags,
    VROverlayFlags::*,
    VROverlayInputMethod::*,
    EVREventType::*
};

use gl::types::*;

use std::ffi::CString;

use std::time::Instant;

use freedesktop_icons::lookup as icon_lookup;

const WIDTH: usize = 2048;
const HEIGHT: usize = 768;

const MOUSE_SCALE: [f32; 2] = [WIDTH as f32, HEIGHT as f32];
const OVERLAY_WIDTH: f32 = 2.0;
const UI_SCALE: f32 = 4.2;

fn icon_path(icon_name: &str) -> Option<String> {
    ["default", "hicolor", "gnome", "oxygen"].iter().find_map(|theme| {
        let path_buf = icon_lookup(icon_name).with_theme(theme).with_cache().find()?;
        path_buf.into_os_string().into_string().ok()
    })
}

fn icon_uri(icon_name: &str) -> Option<String> {
    icon_path(icon_name).map(|path| {
        "file://".to_owned() + &path
    })
}

use font_kit::{
    handle::Handle, source::SystemSource,
};

fn load_system_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let Ok(family) = SystemSource::new().select_family_by_name("sans-serif")
    else { return };

    for (i, handle) in family.fonts().iter().enumerate() {
        let name = format!("System Sans Serif {}", i);

        let buf: Vec<u8> = match handle {
            Handle::Memory { bytes, .. } => bytes.to_vec(),
            Handle::Path { path, .. } => std::fs::read(path).unwrap(),
        };

        fonts.font_data.insert(
            name.to_owned(), egui::FontData::from_owned(buf));

        if let Some(vec) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            vec.push(name.to_owned());
        }

        if let Some(vec) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
            vec.push(name.to_owned());
        }
    }

    ctx.set_fonts(fonts);
}

struct RenderTarget {
    fbo: GLuint,
    tex: GLuint
}

struct PingPongRenderer {
    targets: [RenderTarget; 2],
    current_target: bool,
}

impl RenderTarget {
    fn new(width: usize, height: usize) -> RenderTarget {
        let mut handle = [0];

        unsafe { gl::GenFramebuffers(1, handle.as_mut_ptr()) }
        let fbo = handle[0];

        unsafe { gl::GenTextures(1, handle.as_mut_ptr()) }
        let tex = handle[0];

        unsafe {
            gl::BindTexture(gl::TEXTURE_2D, tex);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAX_LEVEL, 0);
            gl::TexImage2D(gl::TEXTURE_2D, 0, gl::RGBA8 as i32, width as i32, height as i32,
                           0, gl::RGBA, gl::UNSIGNED_BYTE, std::ptr::null());

            gl::BindFramebuffer(gl::FRAMEBUFFER, fbo);
            gl::FramebufferTexture2D(gl::FRAMEBUFFER, gl::COLOR_ATTACHMENT0, gl::TEXTURE_2D, tex, 0);
        }

        RenderTarget { fbo, tex }
    }
}

impl Drop for RenderTarget {
    fn drop(&mut self) {
        unsafe {
            gl::DeleteTextures(1, [self.tex].as_ptr());
            gl::DeleteFramebuffers(1, [self.fbo].as_ptr())
        }
    }
}

impl PingPongRenderer {
    fn new(width: usize, height: usize) -> PingPongRenderer {
        PingPongRenderer {
            targets: [RenderTarget::new(width, height), RenderTarget::new(width, height)],
            current_target: false
        }
    }

    fn current_texture(&self) -> GLuint {
        self.targets[if self.current_target { 0 } else { 1 }].tex
    }

    fn current_framebuffer(&self) -> GLuint {
        self.targets[if self.current_target { 1 } else { 0 }].fbo
    }

    fn flip(&mut self) {
        self.current_target = !self.current_target;
    }
}

#[allow(non_snake_case)]
fn VROverlayFlags_EnableControlBar() -> VROverlayFlags { unsafe { std::mem::transmute((1 << 23) as u32) } }

#[allow(non_snake_case)]
fn VROverlayFlags_EnableControlBarKeyboard() -> VROverlayFlags { unsafe { std::mem::transmute((1 << 24) as u32) } }

#[allow(non_snake_case)]
fn VROverlayFlags_EnableControlBarClose() -> VROverlayFlags { unsafe { std::mem::transmute((1 << 25) as u32) } }

fn overlay() -> std::pin::Pin<&'static mut openvr_sys2::IVROverlay> {
    let overlay_raw = openvr_sys2::VROverlay();
    if overlay_raw.is_null() {
        panic!("Failed to obtain handle to VROverlay");
    }
    unsafe { std::pin::Pin::new_unchecked(&mut *overlay_raw) }
}

fn poll_vr_event(vr_system: *mut openvr_sys2::IVRSystem) -> Option<openvr_sys2::VREvent_t> {
    let mut event = std::mem::MaybeUninit::<openvr_sys2::VREvent_t>::uninit();

    unsafe {
        let vr = std::pin::Pin::new_unchecked(&mut *vr_system);
        if vr.PollNextEvent(event.as_mut_ptr() as *mut _,
                            std::mem::size_of::<openvr_sys2::VREvent_t>() as u32) {
            return Some(event.assume_init());
        }
    }

    None
}

fn poll_vr_overlay_event(handle:  openvr_sys2::VROverlayHandle_t) -> Option<openvr_sys2::VREvent_t> {
    let mut event = std::mem::MaybeUninit::<openvr_sys2::VREvent_t>::uninit();

    unsafe {
        if overlay().PollNextOverlayEvent(
            handle, event.as_mut_ptr() as *mut _,
            std::mem::size_of::<openvr_sys2::VREvent_t>() as u32) {
            return Some(event.assume_init());
        }
    }

    None
}

fn process_vr_event(
    painter: &Painter, input: &mut RawInput,
    event: &openvr_sys2::VREvent_t,
    quit: &mut bool, shown: &mut bool) {
    let event_type = event.eventType.try_into().unwrap_or(VREvent_None);
    match event_type {
        VREvent_Quit => { *quit = true; },
        VREvent_OverlayClosed => { *quit = true; },
        VREvent_MouseMove => {
            input.events.push(
                egui::Event::PointerMoved(egui::pos2(
                    unsafe { event.data.mouse.x } / painter.pixels_per_point,
                    unsafe { HEIGHT as f32 - event.data.mouse.y } / painter.pixels_per_point,
                )));
        },
        VREvent_MouseButtonDown | VREvent_MouseButtonUp => {
            input.events.push(egui::Event::PointerButton {
                pos: egui::pos2(
                    unsafe { event.data.mouse.x } / painter.pixels_per_point,
                    unsafe { HEIGHT as f32 - event.data.mouse.y } / painter.pixels_per_point,
                ),
                button: match unsafe { event.data.mouse.button } {
                    1 => egui::PointerButton::Primary,
                    2 => egui::PointerButton::Secondary,
                    3 => egui::PointerButton::Middle,
                    _ => egui::PointerButton::Extra1,
                },
                pressed: event_type == VREvent_MouseButtonDown,
                modifiers: egui::Modifiers::NONE
            });
        },
        VREvent_ScrollDiscrete | VREvent_ScrollSmooth => {
            input.events.push(egui::Event::Scroll(
                egui::vec2(
                    unsafe { event.data.scroll.xdelta } / painter.pixels_per_point,
                    unsafe { event.data.scroll.ydelta } / painter.pixels_per_point
                )
            ));
        },
        VREvent_KeyboardCharInput => {
            let mut buffer: [u8; 12] = [0; 12];
            for (i, &x) in unsafe { std::ptr::addr_of!(event.data.keyboard).read_unaligned().cNewInput }.iter().enumerate() {
                buffer[i] = x as u8;
            }

            if let Ok(cstr) = std::ffi::CStr::from_bytes_until_nul(&buffer) {
                if let Ok(string) = cstr.to_str() {
                    if string == "\n" {
                        for &val in [true, false].iter() {
                            input.events.push(egui::Event::Key {
                                key: egui::Key::Enter,
                                pressed: val,
                                repeat: false,
                                modifiers: egui::Modifiers::NONE
                            });
                        }
                    }
                    else if string == "\x08" { // Backspace control character
                        for &val in [true, false].iter() {
                            input.events.push(egui::Event::Key {
                                key: egui::Key::Backspace,
                                pressed: val,
                                repeat: false,
                                modifiers: egui::Modifiers::NONE
                            });
                        }
                    }
                    else {
                        input.events.push(egui::Event::Text(string.to_string()))
                    }
                }
            }
        },
        VREvent_FocusEnter => { input.events.push(egui::Event::WindowFocused(true)); },
        VREvent_FocusLeave => { input.events.push(egui::Event::WindowFocused(false)); },
        VREvent_OverlayShown => { *shown = true; }
        VREvent_OverlayHidden => { *shown = false; }

        _ => ()
    }
}

fn find_players(finder: &mpris::PlayerFinder) -> Vec<mpris::Player> {
    finder.
        iter_players().map(|x| x.flatten().collect()).
        unwrap_or_else(|_| vec![])
}

fn main() {
    let finder = mpris::PlayerFinder::new().expect("Failed to connect to DBus MediaPlayer2");

    let mut players: Vec<_> = find_players(&finder);
    let mut last_players_lookup = Instant::now();

    let sdl = sdl2::init().expect("Failed to initialize SDL");

    let mut error = openvr_sys2::EVRInitError::VRInitError_None;
    let vr_system = unsafe {
        openvr_sys2::VR_Init(
            &mut error,
            openvr_sys2::EVRApplicationType::VRApplication_Overlay,
            std::ptr::null())
    };

    if vr_system.is_null() {
        panic!("Failed to initialize OpenVR");
    }

    let sdl_video = sdl.video().expect("Failed to initialize SDL Video");
    let gl_attr = sdl_video.gl_attr();
    gl_attr.set_context_profile(sdl2::video::GLProfile::Core);
    gl_attr.set_context_major_version(3);
    gl_attr.set_context_minor_version(2);

    gl_attr.set_multisample_buffers(0);
    gl_attr.set_multisample_samples(0);
    gl_attr.set_share_with_current_context(true);

    let window = sdl_video.window("mpris-openvr-overlay", 128, 128).
        opengl().hidden().build().expect("Failed to open window");

    let _context = window.gl_create_context().expect("Failed to create OpenGL context");

    gl::load_with(|s| sdl_video.gl_get_proc_address(s) as *const _);

    let mut overlay_handle_slot   = std::mem::MaybeUninit::<openvr_sys2::VROverlayHandle_t>::uninit();
    let mut thumbnail_handle_slot = std::mem::MaybeUninit::<openvr_sys2::VROverlayHandle_t>::uninit();
    unsafe {
        let key = CString::new("mpris-openvr-overlay").unwrap();
        let name = CString::new("Media Player").unwrap();

        overlay().CreateDashboardOverlay(
            key.as_ptr() as *const _,
            name.as_ptr() as *const _,
            overlay_handle_slot.as_mut_ptr(),
            thumbnail_handle_slot.as_mut_ptr());
    }

    let overlay_handle   = unsafe { overlay_handle_slot.assume_init() };
    let thumbnail_handle = unsafe { thumbnail_handle_slot.assume_init() };

    if let Some(path) = icon_path("multimedia-player").and_then(|p| CString::new(p).ok()) {
        unsafe {
            overlay().SetOverlayFromFile(thumbnail_handle, path.as_ptr());
        }
    }

    overlay().SetOverlayInputMethod(overlay_handle, VROverlayInputMethod_Mouse);

    overlay().SetOverlayFlag(overlay_handle, VROverlayFlags_EnableControlBar(), true);
    overlay().SetOverlayFlag(overlay_handle, VROverlayFlags_EnableControlBarClose(), true);
    overlay().SetOverlayFlag(overlay_handle, VROverlayFlags_EnableControlBarKeyboard(), true);
    overlay().SetOverlayFlag(overlay_handle, VROverlayFlags_SendVRSmoothScrollEvents, true);

    unsafe {
        overlay().SetOverlayMouseScale(overlay_handle, MOUSE_SCALE.as_ptr() as *const _);
    }

    overlay().SetOverlayWidthInMeters(overlay_handle, OVERLAY_WIDTH);

    let mut renderer = PingPongRenderer::new(WIDTH, HEIGHT);

    unsafe {
        gl::Disable(gl::DEPTH_TEST);
    }

    let egui_ctxt = egui::Context::default();
    egui_extras::install_image_loaders(&egui_ctxt);

    println!("{:?}", egui_ctxt.style());
    let (mut painter, mut egui_state) =
        egui_sdl2_gl::with_sdl2(&window, ShaderVersion::Default, DpiScaling::Custom(UI_SCALE));
    painter.update_screen_rect((WIDTH as u32, HEIGHT as u32));

    let mut egui_input = RawInput {
        screen_rect: Some(painter.screen_rect),
        pixels_per_point: Some(painter.pixels_per_point),
        ..Default::default()
    };


    let mut quit = false;

    let mut event_pump = sdl.event_pump().expect("Failed to acquire events");

    let start_time = Instant::now();

    let mut selected_player_id = 0;

    let mut metadata = None;
    let mut metadata_last_lookup = Instant::now();

    let mut previous_id = selected_player_id;

    let mut shown = true;

    // load_system_font(&egui_ctxt);

    while !quit {
        if last_players_lookup.elapsed().as_secs_f64() > 3.0 {
            let old_player_bus_name = if selected_player_id >= players.len() {
                None
            } else {
                Some(players[selected_player_id].bus_name().to_string())
            };

            players = find_players(&finder);
            last_players_lookup = Instant::now();

            selected_player_id = 0;

            if let Some(bus) = old_player_bus_name {
                if let Some(id) = players.iter().position(|p| p.bus_name() == bus) {
                    selected_player_id = id;
                }
            }
        }

        egui_state.input.time = Some(start_time.elapsed().as_secs_f64());
        egui_ctxt.begin_frame(egui_input.take());

        if shown && !players.is_empty() {
            if selected_player_id >= players.len() {
                selected_player_id = players.len() - 1;
            }

            let selected_player = &players[selected_player_id];
            let mut volume = selected_player.get_volume().unwrap_or(1.0);
            let old_volume = volume;

            if metadata_last_lookup.elapsed().as_secs_f64() >= 1.0 || previous_id != selected_player_id {
                metadata = selected_player.get_metadata().ok();
                metadata_last_lookup = Instant::now();
                previous_id = selected_player_id;
            }

            if let Some(metadata) = metadata.as_ref() {
                egui::SidePanel::left("icon").show(&egui_ctxt, |ui| {
                    if let Some(url) = metadata.art_url() {
                        ui.add(egui::Image::new(url).show_loading_spinner(true).shrink_to_fit());
                    }
                });
            }

            egui::CentralPanel::default().show(&egui_ctxt, |ui| {
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    egui::ComboBox::from_label("Player").
                        selected_text(players[selected_player_id].identity()).
                        show_index(
                            ui,
                            &mut selected_player_id,
                            players.len(),
                            |i| players[i].identity()
                        );
                });
                ui.separator();

                if let Some(metadata) = metadata.as_ref() {
                    let song_name = metadata.title().unwrap_or("?");
                    let artists = metadata.artists().map(|x| x.join(", ")).map(|x| {
                        if x.is_empty() { x } else { x + " - " }
                    }).unwrap_or("".to_string());
                    let shown_name = artists + song_name;
                    ui.label(shown_name);
                }

                if selected_player.has_volume().unwrap_or(false) {
                    ui.horizontal(|ui| {
                        ui.label("Volume");
                        ui.add(egui::Slider::new(&mut volume, 0.0..=1.0).
                               show_value(false).
                               trailing_fill(true));
                    });
                }

                if selected_player.has_position().unwrap_or(false) {
                    if let Some(metadata) = metadata.as_ref() {
                        let pos = selected_player.get_position().unwrap_or(std::time::Duration::ZERO);
                        let duration = metadata.length().unwrap_or(std::time::Duration::ZERO);
                        let mut out_pos = pos.as_secs_f64();

                        ui.horizontal(|ui| {
                            let time_formatter = |x, _| {
                                let total_secs = x as u64;

                                let secs = total_secs % 60;
                                let minutes = (total_secs / 60) % 60;
                                let hours = (total_secs / 60) / 60;

                                if hours == 0 {
                                    format!("{:0>2}:{:0>2}", minutes, secs)
                                }
                                else {
                                    format!("{:0>2}:{:0>2}:{:0>2}", hours, minutes, secs)
                                }
                            };

                            ui.add(egui::Slider::new(&mut out_pos, 0.0..=duration.as_secs_f64()).
                                   custom_formatter(time_formatter).
                                   trailing_fill(true));
                            ui.label(time_formatter(duration.as_secs_f64(), 0..=0));
                        });

                        if out_pos != pos.as_secs_f64() {
                            if let Some(id) = metadata.track_id() {
                                let _ = selected_player.set_position(id, &std::time::Duration::from_secs_f64(out_pos));
                            }
                        }
                    }
                }

                if volume != old_volume {
                    let _ = selected_player.set_volume(volume);
                }

                ui.separator();

                ui.horizontal(|ui| {
                    let pause = selected_player.get_playback_status().
                        unwrap_or(mpris::PlaybackStatus::Stopped) ==
                        mpris::PlaybackStatus::Playing;

                    if let Some(icon) = icon_uri(if pause  { "media-playback-pause" } else { "media-playback-start" }) {
                        if ui.add(egui::ImageButton::new(egui::Image::from_uri(icon))).clicked() {
                            if pause {
                                let _ = selected_player.pause();
                            }
                            else {
                                let _ = selected_player.play();
                            }
                        }
                    }

                    if selected_player.can_go_previous().unwrap_or(false) {
                        if let Some(icon) = icon_uri("media-skip-backward") {
                            if ui.add(egui::ImageButton::new(icon)).clicked() {
                                let _ = selected_player.previous();
                            }
                        }
                    }

                    if selected_player.can_stop().unwrap_or(false) {
                        if let Some(icon) = icon_uri("media-playback-stop") {
                            if ui.add(egui::ImageButton::new(icon)).clicked() {
                                let _ = selected_player.stop();
                            }
                        }
                    }

                    if selected_player.can_go_next().unwrap_or(false) {
                        if let Some(icon) = icon_uri("media-skip-forward") {
                            if ui.add(egui::ImageButton::new(icon)).clicked() {
                                let _ = selected_player.next();
                            }
                        }
                    }

                    if selected_player.can_shuffle().unwrap_or(false) {
                        let shuffle_state = selected_player.get_shuffle().unwrap_or(false);
                        if let Some(icon) = icon_uri("media-playlist-shuffle") {
                            if ui.add(egui::ImageButton::new(icon).selected(shuffle_state)).clicked() {
                                let _ = selected_player.set_shuffle(!shuffle_state);
                            }
                        }
                    }

                    if selected_player.can_loop().unwrap_or(false) {
                        let loop_state = selected_player.get_loop_status().unwrap_or(mpris::LoopStatus::None);
                        if let Some(icon) = icon_uri("media-playlist-repeat") {
                            if ui.add(egui::ImageButton::new(icon).selected(loop_state != mpris::LoopStatus::None)).clicked() {
                                if loop_state == mpris::LoopStatus::None {
                                    let _ = selected_player.set_loop_status(mpris::LoopStatus::Track);
                                }
                                else {
                                    let _ = selected_player.set_loop_status(mpris::LoopStatus::None);
                                }
                            }
                        }
                    }
                });
            });
        }

        let FullOutput {
            platform_output,
            repaint_after: _,
            textures_delta,
            shapes,
        } = egui_ctxt.end_frame();

        egui_state.process_output(&window, &platform_output);

        if shown {
            let paint_jobs = egui_ctxt.tessellate(shapes);

            unsafe {
                gl::BindFramebuffer(gl::DRAW_FRAMEBUFFER, renderer.current_framebuffer());
                gl::Viewport(0, 0, WIDTH as i32, HEIGHT as i32);
                gl::DrawBuffer(gl::COLOR_ATTACHMENT0);

                gl::ClearColor(0.0, 0.0, 0.0, 0.0);
                gl::Clear(gl::COLOR_BUFFER_BIT);
            }

            painter.paint_jobs(None, textures_delta, paint_jobs);

            let bounds = openvr_sys2::VRTextureBounds_t {
                uMin: 0.0, vMin: 0.0, uMax: 1.0, vMax: 1.0
            };

            let texture = openvr_sys2::Texture_t {
                eType: openvr_sys2::ETextureType::TextureType_OpenGL,
                /* egui_sdl2_gl renders with FRAMEBUFFER_SRGB enabled */
                eColorSpace: openvr_sys2::EColorSpace::ColorSpace_Gamma,
                handle: renderer.current_texture() as usize as *mut std::ffi::c_void
            };

            unsafe { overlay().SetOverlayTexture(overlay_handle, &texture); };
            unsafe { overlay().SetOverlayTextureBounds(overlay_handle, &bounds); };
        }

        for event in event_pump.poll_iter() {
            match event {
                sdl2::event::Event::Quit { .. } => { quit = true; },
                _ => ()
            }
        }

        while let Some(event) = poll_vr_event(vr_system) {
            match event.eventType.try_into().unwrap_or(VREvent_None) {
                VREvent_Quit => { quit = true; },
                _ => ()
            }
        }

        while let Some(event) = poll_vr_overlay_event(overlay_handle){
            process_vr_event(&painter, &mut egui_input, &event,
                             &mut quit, &mut shown);
        }

        unsafe {
            gl::Flush();
        }

        renderer.flip();
        overlay().WaitFrameSync(20);
    }

    openvr_sys2::VR_Shutdown();
}
