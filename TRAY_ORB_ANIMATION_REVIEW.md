# Tray / Orb 动画 Review 报告

生成时间：2026-06-17

范围：

- `zhongshu-orb/src/indicator.rs`
- `zhongshu-orb/src/render.rs`
- `zhongshu-orb/src/handler.rs`
- `zhongshu-orb/src/config.rs`

结论：当前 tray/orb 动画没有 P0 级功能断裂，但存在几个 P1/P2 体验问题。主要原因不是单个颜色或参数不好看，而是 Linux tray 和非 Linux orb 走了两套完全不同的渲染路径，状态动画、刷新节奏、视觉语言和生命周期没有统一。

## P1：非 Linux orb 回到 Idle/Done 后动画会停在单帧

位置：

- `zhongshu-orb/src/indicator.rs`
- `OrbIndicator::render`

现状：

```rust
if !matches!(self.state, AgentState::Idle) {
    self.window.request_redraw();
}
```

问题：

- `render::draw_orb` 对 `Idle` 和 `Done` 也计算了 breath 周期。
- 但 `Idle` 状态不会持续 `request_redraw()`，所以 idle 呼吸只会在创建窗口、状态切换或外部 redraw 时偶发更新。
- `Done` 虽然不是 Idle，会持续重绘，但 app 很快会发布 `Done -> Idle`，完成态很可能只闪一下，用户不一定能看到反馈。
- 颜色 lerp 的 0.3s 过渡在进入 Idle 后也可能只渲染第一帧，看起来像突然变色，而不是平滑过渡。

影响：

- orb 的“呼吸感”不稳定。
- 思考/执行时是活的，结束后突然静止，状态反馈显得硬。

建议：

- 增加一个动画控制层，明确区分：
  - `Idle`: 低帧率持续呼吸，例如 8-12 FPS。
  - `Thinking/Executing`: 24-30 FPS。
  - `Done`: 保持 800-1200ms 成功/失败反馈后再回 Idle，或在 indicator 内做 transient done。
- 不建议让业务层频繁切状态来模拟动画，应由 indicator 自己管理视觉时间线。

## P1：Linux tray 动画依赖高频更新 tray pixmap，效果和性能都不可控

位置：

- `zhongshu-orb/src/indicator.rs`
- `tray::TrayIndicator::create`
- `tray::icon_pixmap`

现状：

```rust
let ms = if is_active { 50 } else { 500 };
tokio::time::sleep(Duration::from_millis(ms)).await;
bp.store((elapsed * 50.0) as u32, Ordering::Relaxed);
let _ = h.update(|_: &mut KsniTray| {}).await;
```

问题：

- Linux tray 使用 KStatusNotifier 图标 pixmap，不是动画画布。
- 高频更新 tray icon 在不同桌面环境下表现差异很大：有的会节流，有的会缓存，有的会闪烁，有的更新延迟明显。
- active 状态 20 FPS 更新 tray 图标，可能带来不必要的 DBus/状态栏刷新负担。
- idle 状态 2 FPS，但图标只有小尺寸 alpha 变化，很多状态栏实际看不出来。

影响：

- 用户看到的动画可能是卡顿、闪烁、完全不动，或者只是不规则跳动。
- 这个问题很难通过调参数彻底解决，因为系统 tray 本身不是为连续动画设计的。

建议：

- Linux 上不要把 tray icon 当主动画载体。
- 更合理的方案：
  - tray 只负责入口、菜单、状态颜色。
  - 另建一个小型 always-on-top orb window 作为可视化动画。
  - tray 点击控制 overlay/orb；orb 承担动画。
- 如果短期不做独立 orb window，建议把 tray 动画降级为低频状态变化：
  - Idle 静态蓝色。
  - Thinking 1s 一次轻微亮度变化。
  - Executing 0.5s 间隔交替两帧。
  - Done 显示成功/失败色 1s 后回 Idle。

## P1：状态视觉语言不统一，用户很难从动画判断当前含义

位置：

- `state_color`
- `render::draw_orb`
- `tray::icon_pixmap`

现状：

- Idle：蓝色
- Thinking：黄色
- Executing：珊瑚色
- Done success：绿色
- Done failure：红色

问题：

- 颜色语义是清楚的，但动画语义不够清楚。
- `Thinking` 和 `Executing` 在 orb renderer 中只差 period、颜色和波动速度，没有稳定的形态差异。
- `Done` 只是换成 done mode，但 done mode 目前没有明显的完成反馈形态。
- Linux tray 的图标没有能量波、highlight、hue shift，和非 Linux orb 的视觉完全不是同一个产品语言。

影响：

- 用户能看出“颜色变了”，但不容易感知“正在想 / 正在执行 / 完成 / 失败”的区别。

建议：

- 建立统一状态动画规范：
  - Idle：慢呼吸，低亮度，稳定。
  - Thinking：柔和环形波纹，速度中等。
  - Executing：更明确的脉冲或旋转切线，代表工具动作。
  - Success：短暂扩散 + 绿色 settle。
  - Failure/Stopped：短暂红色收缩或闪烁一次，不要持续刺激。
- tray 如果不能承载完整动画，也要至少使用同一套颜色和节奏语义。

## P2：orb 尺寸默认 64px，细节密度高，实际显示会糊

位置：

- `zhongshu-orb/src/config.rs`
- `default_orb_size() -> 64`
- `render::draw_orb`

问题：

- `draw_orb` 里包含 glow、highlight、wave、hue shift 等多层效果。
- 默认 64px 下细节太密，透明 glow 和 wave 容易糊成一团。
- tray 甚至还生成 16/22/24px 图标，呼吸和渐变更难被识别。

建议：

- 对小尺寸使用简化渲染：
  - 16-24px：只保留实心核心 + 外圈 alpha。
  - 48-64px：保留 highlight，不建议保留复杂 wave。
  - 96px+：再使用完整 orb renderer。
- 或者把默认 orb 尺寸提升到 72/80px，同时保证点击热区和透明边界合理。

## P2：渲染测试只验证“有像素”，没有验证动画质量

位置：

- `zhongshu-orb/src/render.rs` tests

现状：

- 测试会写 PPM 到 `/tmp/zhongshu_orb_test`。
- 验证中心像素、角落 alpha、呼吸半径变化。

问题：

- 没有检测连续帧差异是否平滑。
- 没有检测不同状态之间是否有足够视觉差异。
- 没有检测小尺寸图标是否仍可识别。

建议：

- 增加纯逻辑测试：
  - 相邻帧像素差异不能过大，避免闪烁。
  - `Thinking` 和 `Executing` 的帧差异应高于 Idle。
  - `Done success` / `Done failure` 和 Idle 的主色差应明显。
- 增加人工 review 输出：
  - 生成 sprite sheet PNG/PPM：每个状态 8-12 帧。
  - 不需要进 CI，但方便本地评审动画。

## P2：完成态生命周期由 Agent 状态驱动，视觉反馈时间不可控

位置：

- `zhongshu-orb/src/app.rs`
- `zhongshu-orb/src/handler.rs`

现状：

- agent 完成后发布 `Done`，然后再发布 `Idle`。
- indicator 只被动接收状态，没有自己的最小展示时间。

问题：

- 如果 `Done -> Idle` 很快，成功/失败反馈几乎不可见。
- 如果后续任务马上开始，状态切换可能看起来跳跃。

建议：

- 在 indicator 层维护 visual state，而不是完全等同于 agent state。
- 例如：
  - agent state = Done 时，visual state 至少保持 900ms。
  - 如果这 900ms 内收到 Thinking/Executing，可以立即切到新 active 状态。
  - 如果没有新状态，自动回 Idle。

## 建议的修复顺序

1. 先统一视觉状态机，不急着改颜色。
   - 加 `VisualState` / `VisualMode`。
   - Done 最小展示时间。
   - Idle 低 FPS 持续呼吸。

2. Linux tray 降级为状态入口，不再追求连续动画。
   - 短期：降低 tray 更新频率，减少卡顿/闪烁。
   - 中期：Linux 也创建独立 orb window，tray 只负责菜单。

3. 调整 renderer 小尺寸策略。
   - 默认 64px 可以保留，但小尺寸 tray icon 应简化。
   - 非 Linux orb 可以考虑 72/80px。

4. 增加动画 review artifact。
   - 生成每个状态的 sprite sheet。
   - 用人工视觉检查确认节奏和形态。

## 不建议做的事

- 不建议只调颜色或 breath 参数。这会改善局部观感，但解决不了 tray 机制和状态生命周期问题。
- 不建议在业务事件里插入 sleep 来延长 Done 状态。视觉时长应该由 indicator 管理，不能阻塞 agent 或 event loop。
- 不建议继续提高 Linux tray 的刷新率。tray icon 高频更新不是可靠动画方案。

## 总体判断

当前实现能表达状态，但还不像一个稳定的桌面产品动画系统。最值得优先修的是状态机和刷新策略：让 orb/tray 的视觉状态有自己的生命周期，再根据平台选择合适载体。Linux tray 的连续动画天然受限，应该尽早把“好看的动画”从 tray icon 迁移到独立 orb window。
