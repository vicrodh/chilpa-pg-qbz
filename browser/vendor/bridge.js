// ZBQ: the bridge into Qobuz's official web player, injected at DocumentReady.
//
// Everything here goes through the official player's OWN action creators (the same
// thunks its play buttons dispatch) and reads its public Redux state. Qobuz's code
// does all request signing, stream fetching and decryption, exactly as when a user
// clicks play in the browser; this script never sees a stream URL, a key, a secret or
// any audio. See COMPLIANCE.md at the repository root.
//
// Surface (window.__zbq):
//   ready()                     -> true once the store and the action modules are found
//   state()                     -> JSON string snapshot (login, playback, queue, quality)
//   play(trackIds, index, type, id)  replace the official queue and start at `index`
//   pause() / resume()
//   seek(ms)                    absolute position in milliseconds
//   setVolume(fraction)         0..1
//   setMaxFormat(formatId)      the official quality preference (5, 6, 7, 27)
//   signOut()                   the official player's own log-out
(function () {
  if (window.__zbq) return;

  var store = null, queueActions = null, playerActions = null;

  // Same discovery Qobuzify uses: walk React's fiber tree from #root to the
  // Provider whose store has getState/subscribe.
  function findStore() {
    try {
      var root = document.getElementById("root");
      if (!root) return null;
      var fibers = [];
      if (root._reactRootContainer && root._reactRootContainer._internalRoot) fibers.push(root._reactRootContainer._internalRoot.current);
      Object.keys(root).forEach(function (k) { if (/^__react(Container|Fiber|InternalInstance)/.test(k)) fibers.push(root[k]); });
      for (var r = 0; r < fibers.length; r++) {
        var seen = new Set(), stack = [fibers[r]], n = 0;
        while (stack.length && n < 80000) {
          n++; var f = stack.pop(); if (!f || seen.has(f)) continue; seen.add(f);
          var mp = f.memoizedProps;
          if (mp) {
            for (var i = 0; i < 2; i++) {
              var v = i === 0 ? mp.store : mp.value;
              if (v && typeof v.getState === "function" && typeof v.subscribe === "function" && typeof v.dispatch === "function") return v;
            }
          }
          if (f.child) stack.push(f.child); if (f.sibling) stack.push(f.sibling);
        }
      }
    } catch (e) {}
    return null;
  }

  function findModules() {
    var mods = window.__zbqMods || [];
    for (var i = 0; i < mods.length; i++) {
      var m = mods[i];
      if (!queueActions && typeof m.replacePlayqueue === "function") queueActions = m;
      if (!playerActions && typeof m.togglePlayPause === "function" && typeof m.setCurrentTrackPosition === "function") playerActions = m;
    }
  }

  var autoplayOff = false;
  function ensure() {
    if (!store) store = findStore();
    if (!queueActions || !playerActions) findModules();
    var ok = !!(store && queueActions && playerActions);
    // The app's queue decides what plays next, so the official player's own
    // autoplay (recommendations after the queue ends) is switched off once.
    if (ok && !autoplayOff && typeof queueActions.setAutoplayMode === "function") {
      autoplayOff = true;
      try { store.dispatch(queueActions.setAutoplayMode({ autoplayMode: false })); } catch (e) { autoplayOff = false; }
    }
    return ok;
  }

  function dispatch(thunk) {
    if (!ensure()) throw new Error("official player not ready");
    return store.dispatch(thunk);
  }

  function snapshot() {
    var out = { ready: ensure(), loggedIn: false };
    // What discovery has found so far: the app logs changes to these, which is
    // how a Qobuz web-player update that breaks the bridge shows up.
    out.found = { store: !!store, queue: !!queueActions, player: !!playerActions, modules: (window.__zbqMods || []).length };
    if (!store) return out;
    try {
      var s = store.getState();
      var user = s.user || {};
      out.loggedIn = !!user.token;
      out.token = user.token || "";
      out.userId = user.id || (user.infos && user.infos.id) || null;
      var p = s.player || {};
      var pos = p.position || {};
      var base = pos.value || 0;
      out.playing = p.playingState === "play";
      out.playingState = p.playingState || "";
      if (out.playing && pos.timestamp) base += Date.now() - pos.timestamp;
      var ct = p.currentTrack || {};
      out.trackId = ct.id != null ? String(ct.id) : "";
      out.durationMs = ct.duration ? Math.round(ct.duration) : 0;
      out.positionMs = out.durationMs ? Math.max(0, Math.min(base, out.durationMs)) : Math.max(0, base);
      // Volume and the quality ceiling live with the current output's settings
      // (defaults: volume 100, max format Hi-Res L2).
      var o = s.outputs || {};
      var cur = o.current && o.dictionnary ? o.dictionnary[o.current] : null;
      var os = (cur && cur.settings) || {};
      out.volume = typeof os.volume === "number" ? os.volume / 100 : 1;
      out.muted = !!os.muted;
      out.maxAudioFormat = os.maxAudioFormat || null;
      out.quality = p.quality || null;
    } catch (e) { out.error = String(e && e.message || e); }
    return out;
  }

  window.__zbq = {
    ready: ensure,
    state: function () { return JSON.stringify(snapshot()); },
    play: function (trackIds, index, type, id) {
      var ids = (trackIds || []).map(function (t) { return Number(t); }).filter(function (t) { return isFinite(t) && t > 0; });
      if (!ids.length) return false;
      dispatch(queueActions.replacePlayqueue({
        trackIds: ids,
        index: Math.max(0, Math.min(index | 0, ids.length - 1)),
        origin: { type: type || "track", id: id ? String(id) : String(ids[0]) },
        unshuffle: true
      }));
      return true;
    },
    pause: function () { dispatch(playerActions.pause()); return true; },
    resume: function () { dispatch(playerActions.play()); return true; },
    seek: function (ms) { dispatch(playerActions.setCurrentTrackPosition({ position: Math.max(0, ms | 0) })); return true; },
    setVolume: function (fraction) {
      var v = Math.max(0, Math.min(1, Number(fraction) || 0));
      dispatch(playerActions.setVolume({ volume: Math.round(v * 100) })); // the player's scale is 0..100
      return true;
    },
    setMaxFormat: function (formatId) {
      if (typeof playerActions.setMaxAudioFormat !== "function") return false;
      dispatch(playerActions.setMaxAudioFormat({ format: formatId | 0 }));
      return true;
    },
    // The same action the player's own "Log out" menu item dispatches: Qobuz's
    // log-out call plus clearing its stored session. If the page is not ready,
    // or a later Qobuz build ignores the action, its stored session is dropped
    // directly so the next sign-in still starts signed out.
    signOut: function () {
      function signedIn() {
        try { return !!(store && store.getState().user || {}).token; } catch (e) { return false; }
      }
      function dropStorage() {
        try { localStorage.clear(); sessionStorage.clear(); } catch (e) {}
        location.reload();
      }
      if (!ensure() && !store) { dropStorage(); return true; }
      try { store.dispatch({ type: "LOGOUT_EPIC" }); } catch (e) {}
      setTimeout(function () { if (signedIn()) dropStorage(); }, 5000);
      return true;
    }
  };
})();
