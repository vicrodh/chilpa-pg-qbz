//! Native account form; rendering never exposes credentials.
use super::*;

impl AccessController {
  pub async fn form(&self, api: &QobuzClient, language: &str) -> Value {
    let spanish = language.starts_with("es");
    let text = |en: &str, es: &str| if spanish { es.to_string() } else { en.to_string() };
    let (connection, mut setup) = match self.connection().await {
      Ok(Some(c)) => {
        let setup = match self.request(&c, "setup", json!({})).await {
          Ok(setup) => setup,
          // The helper started and stopped answering: its reason goes to the notices.
          Err(error) => json!({"phase":"unavailable","unreachable":true,"problem":error}),
        };
        (Some(c), setup)
      }
      Ok(None) => (None, json!({"phase":"unavailable"})),
      Err(error) => (None, json!({"phase":"unavailable","unreachable":true,"problem":error})),
    };
    if matches!(setup["phase"].as_str(), Some("verify" | "ready")) {
      if let Some(connection) = &connection {
        match self.complete_session(api, connection, &setup).await {
          Ok(()) => setup["phase"] = json!("ready"),
          Err(error) => self.state.lock().await.notice = Some(error),
        }
      }
    }
    let connected = api.is_logged_in().await && setup["phase"] == "ready";
    let phase = setup["phase"].as_str().unwrap_or("unavailable");
    let step = match phase {
      "login" | "waiting" => 2,
      "checking" | "verify" | "ready" => 3,
      _ => 1,
    };
    let mut fields = vec![];
    let mut actions = vec![];
    let mut values = json!({});
    let action = |key: &str, en: &str, es: &str, fields: Value| json!({"key":key,"label":text(en,es),"fields":fields});
    let message = match phase {
      "detect" => {
        fields.push(json!({"key":"firefox","label":text("Firefox installation","Instalación de Firefox"),"control":{"type":"path","directory":setup["directory"] == true}}));
        let flatpak = setup["found"]
          .as_array()
          .is_some_and(|items| items.iter().any(|p| p["mode"] == "flatpak"));
        if flatpak {
          fields.push(json!({"key":"mode","label":text("Installation type","Tipo de instalación"),"control":{"type":"select","choices":[{"label":"Firefox","value":"native"},{"label":"Firefox Flatpak","value":"flatpak"}]}}));
        }
        values["mode"] = setup["mode"].clone();
        values["firefox"] = setup["path"].clone();
        if setup["missing_decoder"] == true {
          fields.push(json!({"key":"ffmpeg","label":text("FFmpeg executable (required for audio)","Ejecutable de FFmpeg (necesario para el audio)"),"control":{"type":"path"}}));
        }
        actions.push(action(
          "select",
          "Continue",
          "Continuar",
          json!(["firefox", "mode", "ffmpeg"]),
        ));
        actions.push(action("detect", "Detect again", "Detectar de nuevo", json!([])));
        if setup["missing_decoder"] == true {
          text(
            "FFmpeg was not found. Select its executable to prepare lossless audio.",
            "No se encontró FFmpeg. Selecciona su ejecutable para preparar el audio sin pérdida.",
          )
        } else {
          text(
            "Choose Firefox with the file picker, or install it and detect again.",
            "Selecciona Firefox con el explorador, o instálalo y vuelve a detectar.",
          )
        }
      }
      "login" => {
        actions.push(action(
          "login",
          "Sign in with Firefox",
          "Iniciar sesión con Firefox",
          json!([]),
        ));
        actions.push(action("change", "Choose another Firefox", "Elegir otro Firefox", json!([])));
        if setup["snap"] == true {
          text("Firefox Snap detected. Its Qobuz profile is kept in ~/snap/firefox/common, where Snap lets Firefox open it. Sign in in the window that opens; Chilpa will confirm the session automatically.","Se detectó Firefox Snap. Su perfil de Qobuz se guarda en ~/snap/firefox/common, donde Snap deja que Firefox lo abra. Inicia sesión en la ventana que se abrirá; Chilpa confirmará la sesión automáticamente.")
        } else {
          text("Firefox detected. Sign in in the window that opens; Chilpa will confirm the session automatically.","Firefox detectado. Inicia sesión en la ventana que se abrirá; Chilpa confirmará la sesión automáticamente.")
        }
      }
      "waiting" => {
        actions.push(action("cancel", "Cancel", "Cancelar", json!([])));
        text(
          "Finish signing in in Firefox. This step expires after ten minutes.",
          "Inicia sesión en Firefox. Chilpa conservará la sesión en su perfil propio y comprobará la conexión automáticamente.",
        )
      }
      "checking" => {
        actions.push(action("cancel", "Cancel", "Cancelar", json!([])));
        text(
          "Checking that Firefox retains your Qobuz session for playback.",
          "Comprobando que Firefox conserva tu sesión de Qobuz para reproducir música.",
        )
      }
      "verify" | "ready" if !connected => {
        actions.push(action("verify", "Retry connection", "Reintentar conexión", json!([])));
        actions.push(action("login", "Sign in again", "Volver a iniciar sesión", json!([])));
        text(
          "Firefox confirmed the session, but Qobuz could not validate the account. Retry the connection.",
          "Firefox confirmó la sesión, pero no se pudo validar la cuenta de Qobuz. Reintenta la conexión.",
        )
      }
      "ready" | "verify" => {
        actions.push(json!({"key":"sign_out","label":text("Disconnect","Desconectar"),"fields":[],"destructive":"account"}));
        text(
          "Qobuz is connected. You can now search and play music. Open Qobuz in the sidebar.",
          "Qobuz conectado. Ya puedes buscar y reproducir música. Abre Qobuz en la barra lateral.",
        )
      }
      _ => {
        let host = self.managed.get().cloned().unwrap_or(Value::Null);
        if let Some(command) = host["command"].as_str().filter(|c| !c.is_empty()) {
          fields.push(json!({"key":"permission","label":text("Allow desktop access","Permitir acceso al escritorio"),"control":{"type":"copy"}}));
          values["permission"] = json!(command);
          text("Chilpa Flatpak needs permission to start the browser helper on your desktop. Copy and run this command, then restart Chilpa.","Chilpa Flatpak necesita permiso para iniciar el auxiliar del navegador en tu escritorio. Copia y ejecuta este comando y reinicia Chilpa.")
        } else if host["error"] == "desktop_unsupported_sandbox" {
          text(
            "This sandbox does not support desktop browser setup yet.",
            "Este sandbox aún no admite la configuración del navegador de escritorio.",
          )
        } else if host["error"] == "network_disabled" {
          text("Qobuz has no network access, so its browser helper was not started. Turn off Offline mode or allow the plugin network access, then turn Qobuz off and on in Settings > Plugins.","Qobuz no tiene acceso a la red, así que su auxiliar del navegador no se inició. Desactiva el Modo offline o permite la red al plugin, y luego apaga y enciende Qobuz en Ajustes > Plugins.")
        } else if host["error"] == "desktop_start" {
          text("The browser helper did not start. Turn Qobuz off and on in Settings > Plugins. If it keeps happening, attach plugin-desktop/io.github.vicrodh.qbz/startup.log from Chilpa's data folder (~/.local/share/chilpa on Linux) to your report.","El auxiliar del navegador no arrancó. Apaga y enciende Qobuz en Ajustes > Plugins. Si sigue pasando, adjunta a tu reporte el archivo plugin-desktop/io.github.vicrodh.qbz/startup.log de la carpeta de datos de Chilpa (~/.local/share/chilpa en Linux).")
        } else if setup["unreachable"] == true {
          text("The browser helper started but does not answer. Turn Qobuz off and on in Settings > Plugins.","El auxiliar del navegador arrancó pero no responde. Apaga y enciende Qobuz en Ajustes > Plugins.")
        } else if let Some(code) = host["error"].as_str() {
          text(&format!("The browser helper could not start ({code}). Turn Qobuz off and on in Settings > Plugins."),&format!("El auxiliar del navegador no pudo arrancar ({code}). Apaga y enciende Qobuz en Ajustes > Plugins."))
        } else {
          text("This Chilpa version cannot start the browser helper. Update Chilpa, then restart Qobuz.","Esta versión de Chilpa no puede iniciar el auxiliar del navegador. Actualiza Chilpa y reinicia Qobuz.")
        }
      }
    };
    let mut state = self.state.lock().await;
    let mut notices: Vec<String> = state.notice.iter().cloned().collect();
    if let Some(problem) = setup["problem"].as_str().filter(|p| !p.is_empty()) {
      notices.push(problem.into());
    }
    if let Some(problem) = setup["playback_error"].as_str().filter(|problem| !problem.is_empty()) {
      notices.push(problem.into());
    }
    let mut document = json!({
      "status":{"phase":if connected {"ready"} else if matches!(phase, "waiting" | "checking") {"running"} else {"pending"},"label":text("Qobuz account","Cuenta de Qobuz")},
      "wizard":{"steps":[{"label":"Firefox","state":if step > 1 {"complete"} else {"pending"}},
        {"label":text("Sign in","Iniciar sesión"),"state":if step > 2 {"complete"} else {"pending"}},
        {"label":text("Credentials","Credenciales"),"state":if connected {"complete"} else {"pending"}}],
        "step":step,"title":"Qobuz","description":message,"waiting":matches!(phase, "waiting" | "checking"),"refresh_ms":if matches!(phase, "waiting" | "checking") {2000} else {0}},
      "notices":notices,"form":{"fields":fields,"actions":actions},"values":values});
    if state.last_form != document {
      state.revision += 1;
      state.last_form = document.clone();
    }
    document["revision"] = json!(state.revision);
    document
  }
}
