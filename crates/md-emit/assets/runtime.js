/*
 * Master Design animation runtime.
 *
 * Deliberately small and deliberately dumb. Every piece of interpolation, easing and
 * keyframe merging happened in Rust at export time, where it is tested; what is left
 * here is wiring triggers to animations the browser already knows how to play.
 *
 * That split matters. The alternative — shipping the timeline model to the client and
 * evaluating it per frame — would mean a second implementation of the sampler, in a
 * second language, that has to agree with the first one exactly or the exported site
 * looks different from the editor.
 *
 * No dependencies, no build step. The exporter inlines this file verbatim.
 */
(function () {
  "use strict";

  var payload = document.getElementById("md-animations");
  if (!payload) return;

  var entries;
  try {
    entries = JSON.parse(payload.textContent).entries || [];
  } catch (e) {
    // A malformed payload must not take the page down with it — the design is still
    // there and still readable, it just will not move.
    if (window.console) console.warn("[master-design] could not read animations:", e);
    return;
  }

  var prefersReduced =
    window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  // Touch devices have no hover. Treating a press as hover keeps a card responding to a
  // tap instead of appearing inert.
  var isTouch = window.matchMedia && window.matchMedia("(hover: none)").matches;

  entries.forEach(function (entry) {
    var el = document.querySelector(entry.selector);
    if (!el || !el.animate) return;

    // `reducedMotion` is per timeline, because "respect the preference" is not one
    // behaviour: an entrance should land instantly, a decorative bob should not run at
    // all, and motion carrying meaning should still play.
    if (prefersReduced) {
      if (entry.reducedMotion === "ignore") return;
      if (entry.reducedMotion !== "play") {
        applyFinalState(el, entry);
        return;
      }
    }

    var options = Object.assign({ fill: "both" }, entry.options || {});
    // JSON cannot carry Infinity, so an endless loop arrives as a sentinel string.
    if (options.iterations === "infinite") options.iterations = Infinity;
    var animation;
    try {
      animation = el.animate(entry.keyframes, options);
    } catch (e) {
      if (window.console) console.warn("[master-design] animation rejected:", e);
      return;
    }
    animation.pause();

    wire(el, animation, entry);
  });

  /**
   * Jump to the resting state without animating. Playing at zero duration would still
   * fire animation events and still cost a composite; setting the last keyframe does not.
   */
  function applyFinalState(el, entry) {
    var last = entry.keyframes[entry.keyframes.length - 1];
    if (!last) return;
    Object.keys(last).forEach(function (prop) {
      if (prop === "offset" || prop === "easing" || prop === "composite") return;
      try {
        el.style.setProperty(cssName(prop), last[prop]);
      } catch (e) {
        /* an unknown property is not worth failing over */
      }
    });
  }

  function cssName(prop) {
    return prop.replace(/[A-Z]/g, function (c) {
      return "-" + c.toLowerCase();
    });
  }

  function wire(el, animation, entry) {
    var trigger = entry.trigger || { type: "load" };

    switch (trigger.type) {
      case "load":
        delay(trigger.delay, function () {
          animation.play();
        });
        break;

      case "loop":
        animation.play();
        break;

      case "view":
        observe(el, trigger, animation);
        break;

      case "scroll":
        linkToScroll(el, animation, entry, trigger);
        break;

      case "hover":
        bindReversible(
          el,
          animation,
          isTouch ? ["pointerdown"] : ["pointerenter", "focusin"],
          isTouch ? ["pointerup", "pointercancel"] : ["pointerleave", "focusout"]
        );
        break;

      case "click":
        el.addEventListener("click", function () {
          animation.currentTime = 0;
          animation.play();
        });
        break;

      default:
        animation.play();
    }
  }

  function delay(seconds, fn) {
    if (seconds && seconds > 0) window.setTimeout(fn, seconds * 1000);
    else fn();
  }

  function observe(el, trigger, animation) {
    if (!window.IntersectionObserver) {
      animation.play();
      return;
    }
    var observer = new IntersectionObserver(
      function (records) {
        records.forEach(function (record) {
          if (record.isIntersecting) {
            animation.play();
            if (trigger.once !== false) observer.disconnect();
          } else if (trigger.once === false) {
            animation.currentTime = 0;
            animation.pause();
          }
        });
      },
      { threshold: clamp(trigger.threshold == null ? 0.2 : trigger.threshold, 0, 1) }
    );
    observer.observe(el);
  }

  /**
   * Drive a paused animation from scroll position.
   *
   * ScrollTimeline would do this natively and off the main thread, but support is still
   * uneven enough that a page would animate on some of the browsers we target and not
   * others. Scrubbing a paused animation's currentTime works everywhere and is cheap:
   * the work per frame is one assignment, and the browser still composites the result.
   */
  function linkToScroll(el, animation, entry, trigger) {
    var start = trigger.start == null ? 0 : trigger.start;
    var end = trigger.end == null ? 1 : trigger.end;
    var duration = (entry.options && entry.options.duration) || 1000;
    var ticking = false;
    var visible = true;

    function update() {
      ticking = false;
      if (!visible) return;

      var rect = el.getBoundingClientRect();
      var viewport = window.innerHeight || document.documentElement.clientHeight;

      // 0 when the element's top touches the bottom of the viewport, 1 when its bottom
      // touches the top — so the whole traversal of the screen maps onto the timeline.
      var travelled = viewport - rect.top;
      var total = viewport + rect.height;
      var raw = total > 0 ? travelled / total : 0;

      var span = end - start;
      var progress = span !== 0 ? (raw - start) / span : 0;
      animation.currentTime = clamp(progress, 0, 1) * duration;
    }

    function request() {
      if (ticking) return;
      ticking = true;
      window.requestAnimationFrame(update);
    }

    if (window.IntersectionObserver) {
      // Off-screen elements do not need scrubbing, and a long page can hold a lot of them.
      new IntersectionObserver(
        function (records) {
          records.forEach(function (r) {
            visible = r.isIntersecting;
            if (visible) request();
          });
        },
        { rootMargin: "50% 0px" }
      ).observe(el);
    }

    window.addEventListener("scroll", request, { passive: true });
    window.addEventListener("resize", request, { passive: true });
    update();
  }

  function bindReversible(el, animation, enterEvents, leaveEvents) {
    enterEvents.forEach(function (name) {
      el.addEventListener(name, function () {
        animation.playbackRate = 1;
        animation.play();
      });
    });
    leaveEvents.forEach(function (name) {
      el.addEventListener(name, function () {
        animation.playbackRate = -1;
        animation.play();
      });
    });
  }

  function clamp(v, lo, hi) {
    return v < lo ? lo : v > hi ? hi : v;
  }
})();
