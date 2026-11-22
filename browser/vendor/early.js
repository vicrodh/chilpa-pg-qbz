// ZBQ: injected into Qobuz's official web player at DocumentCreation, BEFORE its
// bundle runs. It only REMEMBERS each module's exports object as webpack defines it
// (every module starts with Object.defineProperty(exports, "__esModule", ...)), so the
// late script can look the player's own action creators up by name. It changes nothing
// the page does: no request is altered, nothing is decrypted or read out of the player.
(function () {
  if (window.__zbqMods) return;
  var mods = [];
  var define = Object.defineProperty;
  Object.defineProperty = function (obj, prop, desc) {
    if (prop === "__esModule" && obj && typeof obj === "object") mods.push(obj);
    return define.call(Object, obj, prop, desc);
  };
  define.call(Object, window, "__zbqMods", { value: mods, enumerable: false });
})();
