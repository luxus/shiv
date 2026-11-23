// HP Bar Builder: styles for jcStani's HP Bars. A style is his stylesheet (read from hpbars.vpk: the game's
// unit_status_v2 plus his rules) with a few more rules after it, packed the way mods/hud/build.py packs a stylesheet.
// The first part runs in Node too (module.exports), for checking the bytes against lib/pano.py.
var HpBars = (function () {
  "use strict";

  var BASE_PAK = "/dl/game/citadel/addons/hpbars.vpk";
  var SHEET = "panorama/styles/unit_status_v2.vcss_c";
  var STYLE_MARK = "/* ---- HP Bar Builder style ---- */";   // where a style's rules start in a stylesheet made here
  // the RED2 block of mods/hud/templates/style.vcss_c: every stylesheet HUD++ ships has this one
  var RED2 = "BTNWS3wWEnTpBphGr/LmPrWQN+cBAAAAAAAAQI8CAAABAAAAAAAAACkAAAAHAAcAtQMAALwDAAAAAAAAAAAAAAAAAAAAAAAAlAIAAJgCAAAhAQAAJAEAAAAAAAAAAAAANgAAAAAAAAApAAAABwAAAAcAAAAJAAAA8P//h21fSW5wdXREZXBlbmRlbmNpZXMAbV9SZWxhdGl2ZUZpbGVuYW1lAHBhbm9yYW1hL3N0eWxlcy9jbGVhbl90b3BiYXIudmNzcwBtX1NlYXJjaFBhdGgAY2l0YWRlbF9hZGRvbnMvY2xlYW5fbWluaW1hcABtX25GaWxlQ1JDAG1fYk9wdGlvbmFsAG1fYkZpbGVFeGlzdHMAbV9iSXNHYW1lRmlsZQBwYW5vcmFtYS9zdHlsZXMvY2xlYW5fdG9wYmFyLmNzcwBtX0FkZGl0aW9uYWxJbnB1dERlcGVuZGVuY2llcwBtX0FyZ3VtZW50RGVwZW5kZW5jaWVzAG1fUGFyYW1ldGVyTmFtZQBfX19PdmVycmlkZUlucHV0RGF0YV9fXwBtX1BhcmFtZXRlclR5cGUAQmluYXJ5QmxvYkFyZwBtX25GaW5nZXJwcmludABtX25GaW5nZXJwcmludERlZmF1bHQAbV9TcGVjaWFsRGVwZW5kZW5jaWVzAG1fU3RyaW5nAFBhbm9yYW1hIFByZXByb2Nlc3NvciBQcmVzZW5jZS9WZXJzaW9uAG1fQ29tcGlsZXJJZGVudGlmaWVyAENvbXBpbGVQYW5vcmFtYQBtX25Vc2VyRGF0YQBQYW5vcmFtYSBTdHlsZSBDb21waWxlciBWZXJzaW9uAG1fQWRkaXRpb25hbFJlbGF0ZWRGaWxlcwBtX0NoaWxkUmVzb3VyY2VMaXN0AG1fV2Vha1JlZmVyZW5jZUxpc3QAbV9TZWFyY2hhYmxlVXNlckRhdGEASXNDaGlsZFJlc291cmNlAG1fU3ViYXNzZXRSZWZlcmVuY2VzAG1fU3ViYXNzZXREZWZpbml0aW9ucwAAIAAAAPD/EwoAAAAGAAAABgAAAAQAAAAEAAAABAAAAAEAAAAAAAAAAgAAAAEAAAACAAAAAwAAAAQAAAAFAAAABgAAAAcAAAAIAAAAAQAAAAkAAAADAAAABAAAAAUAAAA0pe5xBgAAAAcAAAAIAAAACgAAAAAAAAALAAAAAQAAAAwAAAANAAAADgAAAA8AAAAQAAAAEQAAABIAAAACAAAAEwAAABQAAAAVAAAAFgAAABAAAAAXAAAAEwAAABgAAAAVAAAAFgAAABAAAAAJAAAAFwAAABkAAAAAAAAAGgAAAAAAAAAbAAAAAAAAABwAAAAdAAAAHgAAAB8AAAAJCAkGBg8NDg4JBgYMDg0OCAgJBgYPDwgJBgYPDwkGBgwPCAgICQ8BAQDd7v8=";

  // jcStani's layout, in the bar's own pixels
  var TOP = 65;          // #UnitStatus margin-top inside the bar's window
  var BAR_Y = 27.77;     // bar top inside #UnitStatus
  var BAR_X = 56.5;      // bar middle inside #UnitStatus (100 wide; the bar's margin-left 13 moves it right of the middle,
                         // which leaves the ultimate icon and the bar together in the middle)
  var NUM_LINE = 1.2027; // number box height per font size: its box ends where the bar starts
  // where the digits sit in the number's box, per font size (measured on docs/img/hp-bars.png)
  var NUM_TOP = 0.265, NUM_MID = 0.61;
  // The critical text (#CriticalIndicator) is the game's, in the window: its middle at x 80 (the panel is centered
  // with margins -8 and 22, its label 10 short of its right side), its capitals' middle 11.7 under the panel's top at
  // 80. Measured in game with the whole label scaled (rules()), at 8, 12, 16 and 20 px: the word is drawn this much
  // narrower than VALVE Pulp at that font size (fit; the preview takes the straight line between), its middle 0.3 left
  // of x 80 at every size.
  var CRIT = { x: 79.7, top: 80, mid: 11.7, size: 12, fit: [[8, 0.94], [12, 0.97], [16, 0.962], [20, 0.97]] };
  function measured(table, v) {
    if (v <= table[0][0]) return table[0][1];
    for (var i = 1; i < table.length; i++)
      if (v <= table[i][0]) return table[i - 1][1] + (table[i][1] - table[i - 1][1]) * (v - table[i - 1][0]) / (table[i][0] - table[i - 1][0]);
    return table[table.length - 1][1];
  }
  // every option, with jcStani's value: a style is the options that differ
  var DEF = {
    w: 54.18, h: 6.3, enemy: "#FFFF00", ally: "#FFFFFF", fill: "flat", bg: "#000000", bgAlpha: 100,
    outline: 0, outlineColor: "#000000", glow: 0,
    num: "above", numSize: 16.38, numAlly: false, numColor: "#FFFFFF", numOutline: 4, numOutlineColor: "#000000", font: "numericOracle",
    hitNum: false, hitNumColor: "#FFD23F", low: false, lowColor: "#FF3030", deltaColor: "#E62020", deltaAll: false,
    ticks: "off", tickW: 1, tickColor: "#000000", stamina: "dots", pip: 5.04, icon: true,
    // the critical text
    crit: true, critSize: 12, critColor: "#FFFFFF", critGlow: "#FF410D",
    // the bar moved (everything placed by it goes along), and the number, ultimate icon, stamina and critical text
    // moved from there (x right, y down)
    barX: 0, barY: 0, numX: 0, numY: 0, iconX: 0, iconY: 0, staX: 0, staY: 0, critX: 0, critY: 0
  };
  // fill looks: brightness down the bar as [position, brightness], multiplied with the bar color
  var FILLS = {
    flat: null,
    shaded: [[0, 1], [1, 0.58]],
    gloss: [[0, 0.82], [0.42, 1], [0.5, 0.72], [1, 0.56]]
  };
  var PRESETS = [
    { id: "flat", title: "Flat", o: {} },
    { id: "ticks", title: "Ticks", o: { ticks: "all" } },
    { id: "slim", title: "Slim", o: { h: 3.78, numSize: 13.86, pip: 3.78 } },
    { id: "bold", title: "Bold", o: { w: 66.15, h: 9.45, numSize: 20.16, ticks: "large", stamina: "squares" } },
    { id: "block", title: "Block", o: { w: 60.48, h: 12.6, num: "inside", numSize: 13.86, ticks: "large", stamina: "bars" } },
    { id: "outline", title: "Outline", o: { h: 7.56, outline: 1.26, fill: "shaded", ticks: "large", stamina: "squares" } },
    { id: "classic", title: "Classic", o: { w: 60.48, h: 8.19, enemy: "#FF4545", ally: "#5BEF8F", fill: "gloss", outline: 1.26, ticks: "all",
        low: true, lowColor: "#FF9A2E", hitNum: true } },
    { id: "neon", title: "Neon", o: { h: 5.04, enemy: "#FF3B6B", ally: "#3BE8FF", glow: 5, bgAlpha: 55, numColor: "#FFFFFF", numOutline: 3,
        numOutlineColor: "#1B0B2E", font: "numericBlock", numSize: 17.64, stamina: "bars" } },
    { id: "minimal", title: "Minimal", o: { w: 47.88, h: 3.78, num: "right", numSize: 11.34, numOutline: 3, stamina: "off", icon: false, bgAlpha: 70 } }
  ];

  // ---------- bytes ----------
  var CRC = (function () {
    var t = new Uint32Array(256);
    for (var i = 0; i < 256; i++) { var c = i; for (var k = 0; k < 8; k++) c = c & 1 ? 0xEDB88320 ^ (c >>> 1) : c >>> 1; t[i] = c >>> 0; }
    return t;
  })();
  function crc32(u8) { var c = 0xFFFFFFFF; for (var i = 0; i < u8.length; i++) c = CRC[(c ^ u8[i]) & 255] ^ (c >>> 8); return (c ^ 0xFFFFFFFF) >>> 0; }
  function concat(parts) { var n = 0, o = 0; parts.forEach(function (p) { n += p.length; }); var out = new Uint8Array(n); parts.forEach(function (p) { out.set(p, o); o += p.length; }); return out; }
  function b64(s) { var b = atob(s), out = new Uint8Array(b.length); for (var i = 0; i < b.length; i++) out[i] = b.charCodeAt(i); return out; }
  function view(u8) { return new DataView(u8.buffer, u8.byteOffset, u8.byteLength); }

  // a VPK whose files are all inside the one file -> {path: bytes}
  function readVpk(u8) {
    var dv = view(u8), dec = new TextDecoder(), out = {};
    if (dv.getUint32(0, true) !== 0x55AA1234) throw new Error("not a VPK");
    var version = dv.getUint32(4, true), header = version === 2 ? 28 : 12, dataStart = header + dv.getUint32(8, true), p = header;
    function str() { var e = p; while (u8[e]) e++; var s = dec.decode(u8.subarray(p, e)); p = e + 1; return s; }
    for (var ext = str(); ext; ext = str())
      for (var dir = str(); dir; dir = str())
        for (var name = str(); name; name = str()) {
          var preload = dv.getUint16(p + 4, true), archive = dv.getUint16(p + 6, true), off = dv.getUint32(p + 8, true), len = dv.getUint32(p + 12, true);
          p += 18;
          if (archive !== 0x7FFF) throw new Error("VPK split into chunks");
          out[(dir === " " ? "" : dir + "/") + name + "." + ext] = concat([u8.subarray(p, p + preload), u8.subarray(dataStart + off, dataStart + off + len)]);
          p += preload;
        }
    return out;
  }

  // [{path, data}] -> VPK v1, everything in the one file (as the Hitsound Builder writes it)
  function buildVpk(files) {
    var enc = new TextEncoder(), groups = {}, tree = [], blobs = [], offset = 0;
    files.forEach(function (f) {
      var slash = f.path.lastIndexOf("/"), dot = f.path.lastIndexOf(".");
      var ext = f.path.slice(dot + 1), dir = slash < 0 ? " " : f.path.slice(0, slash), name = f.path.slice(slash + 1, dot);
      ((groups[ext] = groups[ext] || {})[dir] = groups[ext][dir] || []).push({ name: name, data: f.data });
    });
    function str(s) { tree.push(enc.encode(s), new Uint8Array(1)); }
    Object.keys(groups).sort().forEach(function (ext) {
      str(ext);
      Object.keys(groups[ext]).sort().forEach(function (dir) {
        str(dir);
        groups[ext][dir].sort(function (a, b) { return a.name < b.name ? -1 : 1; }).forEach(function (f) {
          str(f.name);
          var e = new DataView(new ArrayBuffer(18));
          e.setUint32(0, crc32(f.data), true); e.setUint16(4, 0, true); e.setUint16(6, 0x7FFF, true);
          e.setUint32(8, offset, true); e.setUint32(12, f.data.length, true); e.setUint16(16, 0xFFFF, true);
          tree.push(new Uint8Array(e.buffer)); blobs.push(f.data); offset += f.data.length;
        });
        tree.push(new Uint8Array(1));
      });
      tree.push(new Uint8Array(1));
    });
    tree.push(new Uint8Array(1));
    var t = concat(tree), h = new DataView(new ArrayBuffer(12));
    h.setUint32(0, 0x55AA1234, true); h.setUint32(4, 1, true); h.setUint32(8, t.length, true);
    return concat([new Uint8Array(h.buffer), t].concat(blobs));
  }

  // the blocks of a compiled resource
  function blocks(u8) {
    var dv = view(u8), base = 8 + dv.getUint32(8, true), n = dv.getUint32(12, true), out = [];
    for (var i = 0; i < n; i++) {
      var o = base + i * 12, start = o + 4 + dv.getUint32(o + 4, true);
      out.push({ type: String.fromCharCode(u8[o], u8[o + 1], u8[o + 2], u8[o + 3]), data: u8.subarray(start, start + dv.getUint32(o + 8, true)) });
    }
    return out;
  }

  // the text of a compiled stylesheet: CRC, a list of the files it uses (may be empty), then the text
  function cssText(u8) {
    var d = blocks(u8).filter(function (b) { return b.type === "DATA"; })[0].data, n = view(d).getUint16(4, true), o = 6;
    for (var i = 0; i < n; i++) { while (d[o]) o++; o += 9; }
    return new TextDecoder().decode(d.subarray(o));
  }

  // stylesheet text -> compiled stylesheet: lib/pano.py css_c with mods/hud/templates/style.vcss_c
  function cssC(text) {
    var body = new TextEncoder().encode(text), data = new Uint8Array(6 + body.length);
    view(data).setUint32(0, crc32(body), true);
    data.set(body, 6);
    var list = [["RED2", b64(RED2)], ["DATA", data]], table = 16, pos = table + 12 * list.length, at = [];
    list.forEach(function (b) { pos = Math.ceil(pos / 16) * 16; at.push(pos); pos += b[1].length; });
    var out = new Uint8Array(pos), dv = view(out);
    dv.setUint32(0, pos, true); dv.setUint16(4, 12, true); dv.setUint16(6, 3, true); dv.setUint32(8, 8, true); dv.setUint32(12, list.length, true);
    list.forEach(function (b, i) {
      var e = table + 12 * i;
      for (var k = 0; k < 4; k++) out[e + k] = b[0].charCodeAt(k);
      dv.setUint32(e + 4, at[i] - (e + 4), true); dv.setUint32(e + 8, b[1].length, true);
      out.set(b[1], at[i]);
    });
    return out;
  }

  // ---------- the style ----------
  function options(o) {
    var s = {}, k;
    for (k in DEF) s[k] = o && o[k] !== undefined ? o[k] : DEF[k];
    return s;
  }

  function px(v) { return String(Math.round(v * 100) / 100) + "px"; }
  function hex2(v) { var h = Math.max(0, Math.min(255, Math.round(v))).toString(16).toUpperCase(); return h.length < 2 ? "0" + h : h; }
  function alpha(color, pct) { return pct >= 100 ? color : color + hex2(pct * 2.55); }
  function grey(b) { return "#" + hex2(b * 255) + hex2(b * 255) + hex2(b * 255); }
  // a part moved: a 2D translate, which makes no composition layer and leaves the layout as it is
  function move(x, y) {
    return x || y ? "transform: " + [x ? "translateX(" + px(x) + ")" : "", y ? "translateY(" + px(y) + ")" : ""].join(" ").trim() + ";" : "";
  }

  // where things sit for a style, in the bar's own pixels inside #UnitStatus (the preview draws from the same numbers).
  // The fill keeps jcStani's place; an outline grows around it. The moves are not in these: they are a translate on
  // top. dx, dy is the bar's: on #InfoHealthContainer (the bar, the number and the icon), the stamina and the debuffs.
  // Without the ultimate icon the bar sits in the middle, over the hero.
  function layout(s) {
    var t = s.outline, numTop = s.num === "above" ? BAR_Y - t - NUM_LINE * s.numSize : BAR_Y + s.h / 2 - NUM_MID * s.numSize;
    // debuffs keep jcStani's distance to whatever is under them: the digits, or the bar. A moved number takes them
    // along, but never lower than over the bar
    var digits = numTop + NUM_TOP * s.numSize, under = s.num === "above" ? digits : BAR_Y - t;
    if (s.num !== "off" && s.numY) under = Math.min(BAR_Y - t, digits + s.numY);
    var dx = Math.round((s.barX + (s.icon ? 0 : 50 - BAR_X)) * 100) / 100, dy = s.barY;
    return {
      t: t,
      dx: dx,
      dy: dy,
      numTop: numTop,
      // a centered number's middle is 50 + numLeft / 2: jcStani's over the bar is 3.5 left of the bar's middle, the
      // one inside it is on the middle
      numLeft: s.num === "right" ? BAR_X + s.w / 2 + t + 3 : s.num === "inside" ? 2 * (BAR_X - 50) : 6.07,
      status: 39.81 + under - (BAR_Y - NUM_LINE * DEF.numSize + NUM_TOP * DEF.numSize),
      stamina: TOP + BAR_Y + s.h + t + 2.52,
      iconTop: BAR_Y + s.h / 2 - 9.37,
      iconRight: s.w + 2 * t + 5.9,
      pipW: s.stamina === "bars" ? s.pip * 2.4 : s.pip,
      pipH: s.stamina === "bars" ? s.pip * 0.6 : s.pip,
      // the critical text in the window, with every move: its middle, its size against the game's and how wide the
      // game draws the word (fit, per font size)
      crit: { x: CRIT.x + dx + s.critX, mid: CRIT.top + CRIT.mid + dy + s.critY,
        k: s.critSize / CRIT.size, fit: measured(CRIT.fit, s.critSize) }
    };
  }

  // the rules a style adds after jcStani's (none for his own). Every selector repeats one of his or one of the
  // game's, so a later rule wins; only sizes, margins, colors, shadows and visibility are set.
  function rules(o) {
    var s = options(o), L = layout(s), t = s.outline, out = [];
    function rule(sel, body) { out.push(sel + "{" + body + "}"); }
    // a class the game puts on the bar's root or a panel inside it, the three ways jcStani's rules name one
    function side(cls, x) { return ".player." + cls + " " + x + ",.player ." + cls + " " + x + "," + "." + cls + " .player " + x; }
    function is(k) { return String(s[k]).toUpperCase() === String(DEF[k]).toUpperCase(); }
    var size = !is("w") || !is("h"), box = size || t > 0, V = "#UnitHealthbarValue";

    if (box)
      rule(".player .UnitHealthbarContainer", "width: " + px(s.w + 2 * t) + ";height: " + px(s.h + 2 * t) + ";"
        + (t > 0 ? "margin-top: " + px(BAR_Y - t) + ";background-color: " + s.outlineColor + ";" : ""));
    if (t > 0) rule(side("health_critical", ".ShowCriticalState .UnitHealthbarContainer"), "background-color: " + s.outlineColor + ";");
    if (s.glow > 0)
      [["enemy", s.enemy], ["friend", s.ally]].forEach(function (c) {
        rule(side(c[0], ".UnitHealthbarContainer"), "box-shadow: " + c[1] + "B0 0px 0px " + px(s.glow) + " " + px(s.glow / 4) + ";");
      });
    if (box || !is("bg") || !is("bgAlpha"))
      rule(".player #UnitHealthbarInner", (box ? "width: " + px(s.w) + ";height: " + px(s.h) + ";" : "")
        + (t > 0 ? "margin-top: " + px(t) + ";margin-left: " + px(t) + ";" : "")
        + (!is("bg") || !is("bgAlpha") ? "background-color: " + alpha(s.bg, s.bgAlpha) + ";" : ""));
    if (FILLS[s.fill]) {
      var f = FILLS[s.fill], stops = f.slice(1, -1).map(function (p) { return ", color-stop( " + p[0] + ", " + grey(p[1]) + " )"; }).join("");
      rule(".player #unit_healthbar_lagging,.player #unit_healthbar_delta,.player #unit_healthbar_healing,.player #unit_healthbar_deferred",
        "background-color: gradient( linear, 0% 0%, 0% 100%, from( " + grey(f[0][1]) + " ), to( " + grey(f[f.length - 1][1]) + " )" + stops + " );");
    }
    if (!s.icon) rule(".player .unit_info_panel", "visibility: collapse;");
    else if (box || s.iconX || s.iconY)
      rule(".player .unit_info_panel", (box ? "margin: " + px(L.iconTop) + " " + px(L.iconRight) + " 0px 0px;" : "") + move(s.iconX, s.iconY));

    [["enemy", "enemy"], ["friend", "ally"]].forEach(function (c) {
      if (is(c[1])) return;
      rule(side(c[0], "#unit_healthbar_lagging"), "wash-color: " + s[c[1]] + ";");
      rule(side(c[0], "#unit_ult_ready_icon"), "wash-color: " + s[c[1]] + ";");
      rule(side(c[0], "#StaminaContainer"), "wash-color: " + s[c[1]] + ";");
    });
    if (s.low) rule(side("health_critical", "#unit_healthbar_lagging"), "wash-color: " + s.lowColor + ";");
    if (!is("deltaColor"))
      rule(".player #unit_healthbar_delta," + side("enemy", "#unit_healthbar_delta") + "," + side("friend", "#unit_healthbar_delta"), "wash-color: " + s.deltaColor + ";");
    if (s.deltaAll) rule(".player #unit_healthbar_delta", "opacity: 1;");

    if (s.num === "off") rule(".player " + V, "visibility: collapse;");
    else {
      var n = "";
      if (!is("num") || !is("numSize") || (t > 0 && s.num === "above"))
        n += "font-size: " + px(s.numSize) + ";margin: " + px(L.numTop) + " 0px 0px " + px(L.numLeft) + ";";
      if (s.num === "right") n += "horizontal-align: left;text-align: left;";
      if (!is("font")) n += "font-family: " + s.font + ";";
      if (!is("numOutline") || !is("numOutlineColor"))
        n += s.numOutline > 0 ? "text-shadow: 0px 0px 1px " + s.numOutline.toFixed(1) + " " + s.numOutlineColor + ";" : "text-shadow: none;";
      n += move(s.numX, s.numY);
      if (n) rule(".player " + V, n);
      if (!is("numColor")) rule(".player " + V + "," + side("enemy", V), "color: " + s.numColor + ";");
      if (s.numAlly) rule(side("friend", V), "visibility: visible;");
      if (s.hitNum) rule(side("active_damage_by_local_player", V), "color: " + s.hitNumColor + ";");
    }
    if (L.dx || L.dy) rule(".player #InfoHealthContainer", move(L.dx, L.dy));
    if (Math.abs(L.status - 39.81) > 0.004 || L.dx || L.dy)
      rule(".player #StatusEffects", (Math.abs(L.status - 39.81) > 0.004 ? "margin-top: " + px(L.status) + ";" : "") + move(L.dx, L.dy));

    if (s.ticks !== "off") {
      var line = "width: " + px(s.tickW) + ";background-color: " + s.tickColor + ";opacity: 1;";
      rule(".player #UnitHealthbarLines", "visibility: visible;width: " + px(s.w) + ";height: " + px(s.h) + ";margin-top: " + px(t) + ";margin-left: " + px(t) + ";"
        + 'opacity-mask: url("s2r://panorama/images/masks/no_mask_png.vtex");opacity: 1;');
      rule(".player #UnitHealthbarLines .line_large.line_large", line + "height: 100%;vertical-align: top;");
      rule(".player #UnitHealthbarLines .line_small.line_small", s.ticks === "all" ? line + "height: 50%;vertical-align: bottom;" : "visibility: collapse;");
    }
    // the critical text: its label's animation owns the label's transform, the panel's is free. A new size scales the
    // whole label (padding, margin, max-width too, so the game shrinks the word as much), and the translate keeps
    // its middle where it was
    if (!s.crit) rule(side("health_critical", ".ShowCriticalState #CriticalIndicator"), "visibility: collapse;");
    else {
      var k = L.crit.k, cx = L.dx + s.critX + 5 * (k - 1), cy = L.dy + s.critY - CRIT.mid * (k - 1);
      if (Math.abs(cx) > 0.004 || Math.abs(cy) > 0.004) rule(".player #CriticalIndicator", move(Math.round(cx * 100) / 100, Math.round(cy * 100) / 100));
      var lab = "";
      if (k !== 1) lab += "font-size: " + px(s.critSize) + ";padding: " + px(6 * k) + " " + px(8 * k) + ";margin-right: " + px(10 * k) + ";max-width: " + px(76 * k) + ";";
      if (!is("critColor")) lab += "color: " + s.critColor + ";";
      if (!is("critGlow")) lab += "text-shadow: 0px 0px 5.0 " + s.critGlow + ";";
      if (lab) rule(".player #CriticalIndicator Label", lab);
    }
    if (s.stamina === "off") rule(".player #StaminaContainer", "visibility: collapse;");
    else {
      var sx = L.dx + s.staX, sy = L.dy + s.staY;
      if (box || !is("pip") || s.stamina === "bars" || sx || sy)
        rule(".player #StaminaContainer", (box || !is("pip") || s.stamina === "bars" ? "margin-top: " + px(L.stamina) + ";width: " + px(s.w + 2 * t) + ";height: " + px(L.pipH) + ";" : "")
          + move(sx, sy));
      if (!is("stamina") || !is("pip"))
        rule(".player .StaminaPip .StaminaPipIcon", "width: " + px(L.pipW) + ";height: " + px(L.pipH) + ";border-radius: " + (s.stamina === "dots" ? "50%" : "0px") + ";");
    }
    return out.length ? STYLE_MARK + out.join("") : "";
  }

  // jcStani's pak and a style -> the new pak. The site's pak is itself a style made here (since 2026-11-20): its
  // stylesheet is the base (the game's plus jcStani's rules) up to STYLE_MARK, and a style's rules replace the rest
  function buildPak(basePak, o) {
    var base = readVpk(basePak), sheet = base[SHEET];
    if (!sheet) throw new Error("hpbars.vpk has no " + SHEET);
    var files = [{ path: SHEET, data: cssC(cssText(sheet).split(STYLE_MARK)[0] + rules(o)) }];
    // his pak's mod info goes along (lib/modinfo.py), so a style made here has its name in mod managers too
    ["addoninfo.txt", "modinfo.json"].forEach(function (n) { if (base[n]) files.push({ path: n, data: base[n] }); });
    return buildVpk(files);
  }

  return {
    BASE_PAK: BASE_PAK, SHEET: SHEET, DEF: DEF, PRESETS: PRESETS, FILLS: FILLS, TOP: TOP, BAR_Y: BAR_Y, BAR_X: BAR_X, CRIT: CRIT,
    crc32: crc32, readVpk: readVpk, buildVpk: buildVpk, cssText: cssText, cssC: cssC,
    options: options, layout: layout, rules: rules, buildPak: buildPak
  };
})();

if (typeof module !== "undefined") module.exports = HpBars;
