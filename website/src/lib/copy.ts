// All site text, in every language. Strings may hold inline HTML (<code>, <a>).

export type Language = 'en' | 'zh-CN';

export const sponsorUrl = 'https://github.com/sponsors/huacnlee';
const disktree = 'https://huacnlee.github.io/disktree/';

/** My posts on X about this project, one line each. To add one, add its ID and text. */
export const posts: { id: string; en: string; 'zh-CN': string }[] = [
  { id: '2100501814487691343', en: "Rust's most urgent problem is the disk space <code>target</code> takes.", 'zh-CN': 'Rust 最迫切需要解决的问题是 <code>target</code> 目录磁盘占用问题。' },
  { id: '2108035260508520452', en: "Rust's compile speed and build output have become a serious bottleneck in the AI era.", 'zh-CN': 'Rust 的编译速度和构建产物占用，已成为 AI 时代的严重瓶颈。' },
  { id: '2108102849939075461', en: 'Still, these flaws are no reason to switch languages; that only brings new problems.', 'zh-CN': '不过 Rust 这些缺点，也不是换其他语言的理由，不然只会陷入新的问题里面。' },
  { id: '2109169627226132695', en: 'Cleaning my Windows drive, I found zed and gpui-kit together took 800 GB.', 'zh-CN': '清理了一下 Windows 硬盘，发现之前 zed 和 gpui-kit 加起来占用了 800G。' },
  { id: '2109277995332710700', en: 'After reinstalling Omarchy on NVMe, a few cargo jobs pushed system load past 1000.', 'zh-CN': '重新安装 Omarchy 到 NVMe 的 SSD 盘上，跑了几个 cargo 任务，系统 load 飙到 1000 以上了。' },
  { id: '2109300544049414487', en: 'What filled the I/O: 60 test binaries in one project, nearly 47 GB written per build.', 'zh-CN': '打满 I/O 的原因：某个项目 60 个测试二进制，一次构建光写它们就近 47 GB。' },
  { id: '2113136264157294634', en: 'Tried LayerFS on NSH: a new worktree ran 22 tests in 0.669 s instead of 14.09 s.', 'zh-CN': '在 NSH 上试用 LayerFS：新 worktree 跑完 22 项测试，14.09s 变成 0.669s。' },
  { id: '2115694580620480922', en: '<strong>First release of huacnlee/cargo:</strong> <code>target</code> in one place, shared across worktrees, for less disk and faster builds.', 'zh-CN': '<strong>huacnlee/cargo 第一次发布：</strong>把 <code>target</code> 强制放一起并跟 worktree 共享，减少空间占用，也提升编译速度。' },
  { id: '2115695569574518885', en: 'Comparing stock cargo and huacnlee/cargo: the more worktrees, the bigger the difference.', 'zh-CN': '对比原版 cargo 和 huacnlee/cargo 的差异，worktree 越多越明显。' },
  { id: '2117516009544511706', en: '<strong>Second round of optimization:</strong> keeps <code>target</code> from growing without limit, for AI development across projects and worktrees; never deleted on my MacBook either.', 'zh-CN': '<strong>第二次优化：</strong>控制 <code>target</code> 无限增长，专为 AI 开发、多项目多 worktree 设计；我的 MacBook 上也一直没删过。' },
  { id: '2117517552285044813', en: 'Four days without cleaning <code>target</code>. gpui-kit alone used to reach 100 GB in days; now every project is under control.', 'zh-CN': '已经 4 天没有清理 <code>target</code> 了。以前 gpui-kit 一个项目几天就到 100 GB，现在每个项目都控制下来了。' },
  { id: '2117892529949667444', en: 'Day five: full of worktrees and automated AI development, with no disk problem at all. Nothing rebuilds from scratch, and the computer stays responsive.', 'zh-CN': '今天是第 5 天：电脑打满了 worktree，自动化 AI 开发，根本没遇到磁盘问题。不再全新编译，电脑也不卡了。' },
];

/** Measurements shared by every language. Rows are [series, value]. */
export const worktrees = {
  // Per worktree, in build order: [build seconds, total GB on disk afterwards].
  stock: [[111.4, 6.7], [106.4, 13.4], [108.6, 20.1], [93.8, 26.9], [87.6, 33.6]],
  fork: [[77.2, 3.7], [5.1, 4.0], [4.8, 4.4], [4.8, 4.8], [4.8, 5.1]],
};
export type Series = 'stock' | 'fork' | 'nightly';
export const metricRows: { unit: string; rows: [Series, number][] }[] = [
  { unit: ' s', rows: [['stock', 111.4], ['fork', 77.2]] },
  { unit: ' s', rows: [['stock', 9.1], ['fork', 9.7], ['nightly', 9.2]] },
  { unit: ' s', rows: [['stock', 8.8], ['fork', 3.4], ['nightly', 3.2]] },
];
export const projectRows: { unit: string; rows: [Series, number][] }[] = [
  { unit: ' s', rows: [['stock', 207.9], ['fork', 206.7]] },
  { unit: ' s', rows: [['stock', 513.8], ['fork', 379.2]] },
  { unit: ' s', rows: [['stock', 513.8], ['fork', 16.2]] },
  { unit: ' GB', rows: [['stock', 10.3], ['fork', 4.9]] },
];

export const copy = {
  en: {
    description: 'Stop deleting target. A drop-in Cargo for machines full of git worktrees and AI agents: build output is shared and cleans itself, so nothing rebuilds from scratch. No configuration.',
    nav: { sponsor: 'Sponsor me', why: 'Why', source: 'Source', releases: 'Releases', language: 'Language', theme: 'Theme', main: 'Main navigation' },
    label: "Drop-in replacement for Rust's Cargo",
    title: 'A Rust Cargo that keeps <code>target</code> small on its own',
    lead: 'huacnlee/cargo is a fork of Cargo, the Rust build tool. Install it and it replaces your <code>cargo</code>, with nothing to configure. Git worktrees and projects share build output that stays under a size limit on its own, so you can run as many AI agents as you like, stop deleting <code>target</code>, and stop rebuilding from scratch.',
    platform: 'Platform', unix: 'macOS and Linux', windows: 'Windows',
    copyCommand: 'Copy command', copied: 'Copied', copyFailed: 'Copy failed. Select the command and copy it.',
    installNote: 'Installs over <code>~/.cargo/bin/cargo</code> and keeps the rustup proxy as <code>cargo.rustup</code>. <code>cargo +nightly</code> and <code>rust-toolchain.toml</code> keep working. Run the script with <code>--uninstall</code> to put the proxy back.',
    updateNote: '<strong>Install once, stay current.</strong> At most once a day, after a command, cargo checks for a new release in the background and installs it: checksum verified, renamed into place, nothing printed. The last result is in <code>~/.cargo/.fork-update.log</code>. Set <code>CARGO_FORK_AUTO_UPDATE=0</code> to turn it off; it never runs in CI.',

    whyTitle: 'Why I built this',
    whyShort: (href: string) => `Rust is one of the most valuable languages of the AI era, but <code>target</code> became my biggest headache: a 2 TB drive was not enough. The tools I tried did not solve it, so I changed Cargo itself. <a href="${href}">Read the background</a>.`,

    storyTitle: 'Day five, nothing deleted',
    story: 'It now runs on four of my computers: two Linux, one macOS and one Windows, which has seen less use. On Linux and macOS it has run for five days straight, full of worktrees doing automated AI development. I have not cleaned a <code>target</code> directory once, and the disk has not filled up again.',
    shotAlt: 'Disktree treemap of a home directory: ~/.cargo/targets is 41.7 GB of 99.5 GB, with 711.9 GB of the disk free.',
    shotCaption: `My home directory on Omarchy, November 4, 2026, in <a href="${disktree}">Disktree</a>. All Rust build output is 41.7 GB. All my checkouts and worktrees, in <code>work</code>, are 9.9 GB. 711.9 GB of the disk is free.`,
    storyPoints: [
      '<strong>Worktrees stop adding up.</strong> In a 525-crate workspace, each new worktree adds about 0.4 GB instead of 6.7 GB.',
      '<strong>The disk looks after itself.</strong> Each project keeps to twice its largest build of the last day. There is no cron job and no <code>cargo clean</code> to remember.',
      '<strong>Nothing builds from scratch.</strong> Since the build directory stays, a new worktree of the same commit builds in 5 s instead of 106 s, and the machine stays usable while agents work.',
    ],
    tradeoff: 'Rust still compiles slower than languages like Go. That is a trade I accept: we wait a little, and users get the faster app.',
    postsLink: (postsHref: string) => `I also <a href="${postsHref}">wrote about this on X</a>.`,
    sponsorText: 'If this helps you, please sponsor me.',
    sponsorButton: 'Sponsor on GitHub',

    measuredTitle: 'Every new worktree costs less',
    proofTitle: (n: number) => `${n} git worktrees of a 525-crate Rust workspace, each built once. Measured.`,
    diskUsed: 'Disk used', buildTime: 'Build time',
    diskCmp: (stock: string, ratio: string) => `vs ${stock} with Cargo, ${ratio}× less`,
    timeCmp: (stock: string, ratio: string) => `vs ${stock} with Cargo, ${ratio}× faster`,
    setup: 'nsh, a 525-crate workspace, dev build of one binary per worktree on an 8-core Apple Silicon Mac, Rust 1.99 stable. Five worktrees of the same commit in one run; edits are the median of three runs. Compiler caches such as sccache off.',
    series: { stock: 'Cargo', fork: 'Fork', nightly: 'Fork, nightly' },
    thisFork: 'This fork',
    totalDisk: 'Total disk', totalTime: 'Total build time', minutes: 'min',
    measuredNote: 'Each worktree is a fresh checkout building one binary. Cargo gives every worktree its own <code>target</code> directory and compiles all dependencies again. The fork compiles them once and reuses them, so later worktrees only build the workspace crates.',
    metrics: [
      { title: 'One worktree, first build', note: 'Empty build directory. Most of the difference is debug info that dependencies no longer carry.' },
      { title: 'Rebuild after editing a core crate', note: 'Edit nsh-core, which most of the workspace depends on, in the second of two worktrees. About as fast as Cargo, the first edit in a new worktree included; Cranelift on nightly.' },
      { title: 'Undo the edit', note: 'The files match a build made before, so the fork reuses it; Cargo compiles the crates again.' },
    ],

    projectsTitle: 'Projects share dependencies too',
    projectsSetup: 'Release builds of Cargo 1.99.0 and this fork, run one after another on the same machine: nsh, then mado (a 330-crate app in its own repository), then mado again from an empty build directory, as a new worktree or after cargo clean would start. One run each.',
    projects: [
      { title: 'nsh, first build', note: 'Empty build directories. The same speed; dependencies now land in the shared directory.' },
      { title: 'mado, first build after nsh', note: 'mado builds its dependencies at opt-level 3 and nsh at 0, so the two share almost nothing (1 of 330 crates). The fork is faster because dependencies carry no debug info.' },
      { title: 'mado again, empty build directory', note: 'Cargo compiles all 330 crates again, as in its first build. The fork finds the dependencies in the shared directory and compiles 2 crates.' },
      { title: 'Disk after nsh and mado', note: 'Cargo: both target directories. Fork: both project directories (2.3 GB and 0.4 GB) and the shared dependencies (2.2 GB).' },
    ],

    cleanupTitle: 'What a cleanup looks like',
    cleanup: 'After a build, at most once an hour, Cargo removes what no build has used for a day, then the least recently used units until the project fits. It prints one line when it frees something.',

    changesTitle: 'What changes',
    changesIntro: 'Your projects, <code>Cargo.toml</code> and <code>.cargo/config.toml</code> stay as they are.',
    changes: [
      '<strong>One build directory per project.</strong> Intermediate files go to <code>~/.cargo/targets/&lt;name&gt;-&lt;hash&gt;/</code>, where the hash comes from the repository\'s <code>.git</code> directory. Every worktree lands there wherever it is created, reuses compiled dependencies, and only builds its own crates. Binaries still appear in <code>./target</code>.',
      '<strong>Fixed directories.</strong> <code>./target</code> and <code>~/.cargo/targets</code> cannot be moved: <code>--target-dir</code>, <code>CARGO_TARGET_DIR</code>, <code>build.target-dir</code> and <code>build.build-dir</code> are ignored with a warning, so no flag, env var or tool can scatter build output elsewhere.',
      '<strong>Dependencies shared by every project.</strong> Registry and git dependencies build once in <code>~/.cargo/targets/shared-deps/</code> and are reused by any project that needs the same version, features and profile, like Go\'s build cache. <code>cargo check</code>, clippy, rust-analyzer and <code>cargo doc</code> use the same builds, so a check after a build only looks at your own crates.',
      '<strong>Your own crates shared by worktrees too.</strong> A workspace crate is keyed by the content of its files instead of the checkout it is in, so a new worktree of the same commit reuses every crate another worktree built, and switching back to a branch reuses what it built before. Editing a test or a binary rebuilds only that target. Crates with a build script, and crates that read their own path (<code>env!("CARGO_MANIFEST_DIR")</code>), still build per worktree. Worktrees also share rustc\'s incremental caches, so the first edit in a new worktree is incremental too. For this rustc records <code>.</code> as the compile directory (<code>-Zremap-cwd-prefix=.</code>; on stable through <code>RUSTC_BOOTSTRAP=1</code> with no <code>#![feature]</code> allowed), so start a debugger from the workspace root.',
      '<strong>A size limit.</strong> Each project keeps to twice its largest build of the last day, and its units unused for a day are removed. Shared dependencies are kept for 7 days and keep to 50 GiB; a project no build has used for 7 days is removed whole. The units an edit replaces are removed right away, unless another worktree uses them. Older builds of a shared dependency that was rebuilt for new dependencies go after a day, and so do stale test directories in <code>CARGO_TARGET_TMPDIR</code> and binaries in <code>./target</code> whose build unit is gone. Nothing a running build has locked is removed.',
      '<strong>No duplicate object files on macOS.</strong> Debuggers and backtraces read the objects inside each rlib, so the loose copies beside it are removed.',
      '<strong>Worktrees build at the same time.</strong> Fine-grained locking is on, so parallel agents do not queue on one lock. When two builds need the same dependency at once, one compiles it and the other reuses the result.',
      '<strong>Less debug info in dev builds.</strong> Line tables for your crates, none for dependencies. Backtraces keep file and line. A <code>debug</code> setting in your profile still wins.',
      '<strong>Metadata stored once on nightly.</strong> An rlib no longer carries a second copy of its <code>.rmeta</code> on a nightly rustc, about 30% off the shared dependencies.',
      '<strong>Updates itself.</strong> Once a day at most, a background check installs the latest release, so you never rerun the installer. <code>CARGO_FORK_AUTO_UPDATE=0</code> turns it off.',
      '<strong>Cranelift on nightly.</strong> <code>cargo build</code> compiles your crates with Cranelift and runs the compiler front end on up to 8 threads. Tests and release builds keep LLVM. The installer adds the Cranelift component when you have a nightly toolchain.',
    ],
    changeDefault: 'Change a default',
    envVars: 'Set <code>CARGO_SHARED_DEPS=0</code> to keep dependencies in the project\'s build directory, <code>CARGO_CHECK_SHARED_DEPS=0</code> to only check them in <code>cargo check</code>, <code>CARGO_SHARE_WORKSPACE_UNITS=0</code> to build your crates per worktree, <code>CARGO_DEV_CRANELIFT=0</code> to keep LLVM on nightly, and <code>CARGO_RUSTC_THREADS=1</code> for a single-threaded front end.',

    footer: 'Based on <a href="https://github.com/rust-lang/cargo">rust-lang/cargo</a>. Not affiliated with the Rust project.',
    moreProjects: 'More projects',

    why: {
      description: 'The background of huacnlee/cargo: why I changed Cargo itself to end the target directory problem, and how it is designed.',
      before: [
        'Rust is one of the most valuable languages of the AI era: its performance and stability let the apps I build reach users at a very high quality. But compile times and the size of <code>target</code> have troubled me for as long as I have used it.',
        'In the AI era, disk use is a serious problem, and with many worktrees it became my biggest headache. I bought a 2 TB drive for it and that still was not enough, so it got in my way every day. I kept looking for tools and tried kache, mdx and others, but none of them really solved it.',
        'I don\'t like being held back by limits, and I believed I could solve this. All of it runs on Cargo, so I went straight to Cargo itself and reshaped it around my own ideas and the way we build in the AI era.',
        'It follows how I design products: convention over configuration, the idea from DHH. Developers should not have to think about this at all. Tools like kache and mdx ask for tedious setup; this asks for none. Install it, it replaces the <code>cargo</code> on your computer, and its defaults are tuned for the vast majority of cases.',
        'These changes are aggressive. I don\'t expect upstream Cargo to accept them, and I don\'t have much energy to push for that. Upstream has to consider far more, such as support across every platform; I only consider development on macOS, Linux and Windows. So I have fewer worries, and I can go straight at the real problems I run into.',
        'I now run it on four computers: two Linux, one macOS and one Windows. Windows has seen less use, but on Linux and macOS it has run for five days straight. I have not cleaned a <code>target</code> directory once, and my disk has not filled up again, as the screenshot below shows. For me the <code>target</code> disk problem is solved.',
      ],
      shotCaption: `My home directory on Omarchy, November 4, 2026, in <a href="${disktree}">Disktree</a>.`,
      after: [
        'What remains is compile speed, which is still somewhat slower than languages like Go. That is a trade I accept: we wait a little, and users get the faster app. And this fork helps there too. Since nothing forces you to delete <code>target</code> anymore, builds no longer start from nothing, and what was compiled before is still there to reuse.',
      ],
      postsTitle: 'My posts on X along the way',
      more: (home: string) => `See <a href="${home}#changes">what changes</a> and <a href="${home}#measured">the measurements</a>.`,
    },
  },

  'zh-CN': {
    description: '不用再删 target 了。可以直接替换 Cargo，为跑满 git worktree 和 AI Agent 的电脑而做：构建产物共享并自动清理，不再从头编译，无需配置。',
    nav: { sponsor: '赞助作者', why: '背景', source: '源码', releases: '版本', language: '语言', theme: '主题', main: '主导航' },
    label: 'Rust Cargo 的直接替代品',
    title: '自动控制 <code>target</code> 大小的 Rust Cargo',
    lead: 'huacnlee/cargo 是 Rust 构建工具 Cargo 的分支。安装后直接替换你的 <code>cargo</code>，不需要任何配置。git worktree 和项目之间共享构建产物，并自动控制在大小限制之内，所以尽管开 AI Agent，不用再删 <code>target</code>，也不会再从头编译。',
    platform: '平台', unix: 'macOS 和 Linux', windows: 'Windows',
    copyCommand: '复制命令', copied: '已复制', copyFailed: '复制失败，请手动选中命令复制。',
    installNote: '安装时会覆盖 <code>~/.cargo/bin/cargo</code>，并把 rustup 代理保留为 <code>cargo.rustup</code>。<code>cargo +nightly</code> 和 <code>rust-toolchain.toml</code> 照常可用。用 <code>--uninstall</code> 运行脚本可以恢复代理。',
    updateNote: '<strong>安装一次，自动更新。</strong>执行命令后，cargo 最多每天一次在后台检查新版本并安装：校验 checksum，重命名替换，不输出任何内容。最近一次的结果在 <code>~/.cargo/.fork-update.log</code>。设置 <code>CARGO_FORK_AUTO_UPDATE=0</code> 可以关闭；在 CI 中不会运行。',

    whyTitle: '为什么做这个项目',
    whyShort: (href: string) => `Rust 是 AI 时代非常有价值的编程语言，但 <code>target</code> 成了我最头疼的问题：买了 2T 的硬盘都不够。试过的工具都没有解决，所以我直接改造了 Cargo。<a href="${href}">阅读完整背景</a>。`,

    storyTitle: '第 5 天，没有手动删过任何东西',
    story: '它现在跑在我的四台电脑上：两台 Linux、一台 macOS、一台 Windows（Windows 试得比较少）。在 Linux 和 macOS 上已经连续用了 5 天，跑满 worktree 做自动化 AI 开发。我再也没有清理过 <code>target</code> 目录，磁盘也再没有占满过。',
    shotAlt: 'Disktree 中主目录的磁盘占用图：~/.cargo/targets 占 99.5 GB 中的 41.7 GB，磁盘剩余 711.9 GB。',
    shotCaption: `2026 年 11 月 4 日，我在 Omarchy 上的主目录，用 <a href="${disktree}">Disktree</a> 查看。全部 Rust 构建产物 41.7 GB，所有代码和 worktree（<code>work</code> 目录）9.9 GB，磁盘剩余 711.9 GB。`,
    storyPoints: [
      '<strong>worktree 不再累加磁盘。</strong>在 525 个 crate 的 workspace 里，每个新 worktree 只增加约 0.4 GB，而不是 6.7 GB。',
      '<strong>磁盘自己管理自己。</strong>每个项目保持在最近一天最大构建的两倍以内。不需要定时任务，也不用记得 <code>cargo clean</code>。',
      '<strong>不再从头编译。</strong>构建目录一直在，同一 commit 的新 worktree 只要 5 秒，而不是 106 秒。Agent 工作时电脑也不卡。',
    ],
    tradeoff: 'Rust 的编译速度还是比 Go 这类语言慢一些。这个取舍我接受：我们慢一点，用户用起来更快。',
    postsLink: (postsHref: string) => `我也<a href="${postsHref}">在 X 上聊过这件事</a>。`,
    sponsorText: '如果这个项目帮到了你，请赞助我。',
    sponsorButton: '在 GitHub 上赞助',

    measuredTitle: '每个新 worktree 的成本都更低',
    proofTitle: (n: number) => `525 个 crate 的 Rust workspace，${n} 个 git worktree，各构建一次。实测数据。`,
    diskUsed: '磁盘占用', buildTime: '构建时间',
    diskCmp: (stock: string, ratio: string) => `Cargo 为 ${stock}，减少到 1/${ratio}`,
    timeCmp: (stock: string, ratio: string) => `Cargo 为 ${stock}，快 ${ratio} 倍`,
    setup: 'nsh 是一个 525 个 crate 的 workspace，每个 worktree 以 dev 配置构建一个二进制，8 核 Apple Silicon Mac，Rust 1.99 stable。同一 commit 的五个 worktree 跑一轮；编辑相关的数据取三次的中位数。关闭 sccache 等编译器缓存。',
    series: { stock: 'Cargo', fork: 'Fork', nightly: 'Fork，nightly' },
    thisFork: '这个 fork',
    totalDisk: '总磁盘占用', totalTime: '总构建时间', minutes: '分钟',
    measuredNote: '每个 worktree 都是全新检出，构建一个二进制。Cargo 给每个 worktree 单独的 <code>target</code> 目录，并重新编译所有依赖。这个 fork 只编译一次并复用，所以之后的 worktree 只构建 workspace 里的 crate。',
    metrics: [
      { title: '一个 worktree，首次构建', note: '构建目录为空。差距主要来自依赖不再带调试信息。' },
      { title: '编辑核心 crate 后重新构建', note: '在两个 worktree 的第二个里编辑 nsh-core，workspace 大部分都依赖它。速度和 Cargo 差不多，新 worktree 的首次编辑也一样；nightly 下使用 Cranelift。' },
      { title: '撤销编辑', note: '文件和之前的一次构建相同，所以 fork 直接复用；Cargo 则重新编译这些 crate。' },
    ],

    projectsTitle: '不同项目也共享依赖',
    projectsSetup: 'Cargo 1.99.0 和这个 fork 的 release 构建，在同一台机器上依次运行：nsh，然后 mado（独立仓库中 330 个 crate 的应用），再从空的构建目录构建一次 mado，相当于新 worktree 或 cargo clean 之后的情况。各跑一轮。',
    projects: [
      { title: 'nsh，首次构建', note: '构建目录为空。速度相同；依赖现在放进了共享目录。' },
      { title: 'nsh 之后首次构建 mado', note: 'mado 的依赖用 opt-level 3 构建，nsh 用 0，所以两者几乎不共享（330 个 crate 中只有 1 个）。fork 更快是因为依赖不带调试信息。' },
      { title: '再次构建 mado，构建目录为空', note: 'Cargo 和首次构建一样重新编译全部 330 个 crate。fork 在共享目录中找到依赖，只编译 2 个 crate。' },
      { title: 'nsh 和 mado 之后的磁盘占用', note: 'Cargo：两个 target 目录。Fork：两个项目目录（2.3 GB 和 0.4 GB）加共享依赖（2.2 GB）。' },
    ],

    cleanupTitle: '清理是什么样的',
    cleanup: '构建之后，最多每小时一次，Cargo 删除一天内没有构建用过的内容，再按最近最少使用删除构建单元，直到项目符合限制。释放了空间时只输出一行。',

    changesTitle: '有哪些改动',
    changesIntro: '你的项目、<code>Cargo.toml</code> 和 <code>.cargo/config.toml</code> 都不用改。',
    changes: [
      '<strong>每个项目一个构建目录。</strong>中间文件放在 <code>~/.cargo/targets/&lt;name&gt;-&lt;hash&gt;/</code>，hash 来自仓库的 <code>.git</code> 目录。无论 worktree 建在哪里，都落在这个目录，复用已编译的依赖，只构建自己的 crate。二进制仍然出现在 <code>./target</code>。',
      '<strong>目录固定。</strong><code>./target</code> 和 <code>~/.cargo/targets</code> 不能移动：<code>--target-dir</code>、<code>CARGO_TARGET_DIR</code>、<code>build.target-dir</code> 和 <code>build.build-dir</code> 会被忽略并给出警告，所以任何参数、环境变量或工具都无法把构建产物散落到别处。',
      '<strong>所有项目共享依赖。</strong>registry 和 git 依赖在 <code>~/.cargo/targets/shared-deps/</code> 只构建一次，任何需要相同版本、feature 和 profile 的项目都能复用，类似 Go 的构建缓存。<code>cargo check</code>、clippy、rust-analyzer 和 <code>cargo doc</code> 使用同一批构建，所以构建之后的 check 只看你自己的 crate。',
      '<strong>worktree 之间也共享你自己的 crate。</strong>workspace crate 以文件内容而不是所在目录作为 key，所以同一 commit 的新 worktree 能复用其他 worktree 构建好的所有 crate，切回某个分支也能复用之前的构建。编辑测试或二进制时只重新构建那个 target。带 build script 的 crate，以及读取自身路径（<code>env!("CARGO_MANIFEST_DIR")</code>）的 crate，仍然按 worktree 构建。worktree 之间还共享 rustc 的增量缓存，所以新 worktree 的首次编辑也是增量的。为此 rustc 把 <code>.</code> 记录为编译目录（<code>-Zremap-cwd-prefix=.</code>；stable 上通过 <code>RUSTC_BOOTSTRAP=1</code>，且不允许 <code>#![feature]</code>），所以调试器要从 workspace 根目录启动。',
      '<strong>大小限制。</strong>每个项目保持在最近一天最大构建的两倍以内，一天未使用的构建单元会被删除。共享依赖保留 7 天，上限 50 GiB；7 天没有构建用过的项目会被整个删除。编辑替换掉的构建单元会立即删除，除非其他 worktree 还在用。因新依赖而重新构建的共享依赖，其旧版本一天后删除；<code>CARGO_TARGET_TMPDIR</code> 中过期的测试目录，以及 <code>./target</code> 中构建单元已不存在的二进制，也是一天后删除。正在运行的构建锁定的内容不会被删除。',
      '<strong>macOS 上没有重复的目标文件。</strong>调试器和 backtrace 读取 rlib 内部的目标文件，所以旁边的散落副本会被删除。',
      '<strong>worktree 可以同时构建。</strong>开启了细粒度锁，并行的 Agent 不会排队等同一把锁。两个构建同时需要同一个依赖时，一个编译，另一个复用结果。',
      '<strong>dev 构建的调试信息更少。</strong>你的 crate 保留行号表，依赖不带调试信息。backtrace 仍有文件和行号。profile 中的 <code>debug</code> 设置优先。',
      '<strong>nightly 上元数据只存一份。</strong>在 nightly rustc 上，rlib 不再带第二份 <code>.rmeta</code>，共享依赖可减少约 30%。',
      '<strong>自动更新。</strong>最多每天一次，后台检查并安装最新版本，不用重新运行安装脚本。<code>CARGO_FORK_AUTO_UPDATE=0</code> 可以关闭。',
      '<strong>nightly 上使用 Cranelift。</strong><code>cargo build</code> 用 Cranelift 编译你的 crate，编译器前端最多使用 8 个线程。测试和 release 构建仍用 LLVM。如果你有 nightly 工具链，安装脚本会添加 Cranelift 组件。',
    ],
    changeDefault: '修改默认设置',
    envVars: '设置 <code>CARGO_SHARED_DEPS=0</code> 把依赖留在项目的构建目录，<code>CARGO_CHECK_SHARED_DEPS=0</code> 在 <code>cargo check</code> 中只 check 依赖，<code>CARGO_SHARE_WORKSPACE_UNITS=0</code> 按 worktree 构建你的 crate，<code>CARGO_DEV_CRANELIFT=0</code> 在 nightly 上保留 LLVM，<code>CARGO_RUSTC_THREADS=1</code> 让前端单线程运行。',

    footer: '基于 <a href="https://github.com/rust-lang/cargo">rust-lang/cargo</a>，与 Rust 项目没有关联。',
    moreProjects: '更多项目',

    why: {
      description: 'huacnlee/cargo 的项目背景：为什么我直接改造 Cargo 来解决 target 目录的问题，以及它的设计思路。',
      before: [
        '我认为 Rust 是当前 AI 时代非常有价值的编程语言：它有很好的性能和稳定性，能让我们做出的应用以很高的质量交付给用户。但 Rust 的编译速度和 <code>target</code> 目录占用，一直非常困扰我。',
        '<code>target</code> 占用磁盘是当下 AI 时代非常严峻的问题，尤其在多个 worktree 上开发时，它已经是最让人头疼的事情。我为此买了 2T 的硬盘都还不够，每天都受这件事困扰。我不断寻找各类工具，kache、mdx 等都试过，但都没有真正解决我的问题。',
        '而我是那种不受约束的人，我觉得我可以解决这个问题。既然大家都基于 Cargo，我就直接从 Cargo 本身下手，按照我的设想，结合 AI 时代的特点改造了它。',
        '它遵循我的产品设计方式：约定优于配置（Convention over Configuration），来自 DHH 的设计思想。我认为开发者在使用 Cargo 时，本就不应该操心这种事情。kache、mdx 这类工具都需要一些繁琐的配置，我不这么做，我提供的是开箱即用：你只需要安装，它会直接替换掉你电脑上的 <code>cargo</code>，并提供一套适合绝大多数场景的默认配置和调优。',
        '当然，这些改动非常激进，我不期望它们会被上游 Cargo 接受，也没有太多精力去推动这件事。上游往往要考虑得更全面，比如各种平台上的支持，而我只考虑 macOS、Linux 和 Windows 的开发支持。所以我没有那么多顾虑，可以更放手地去解决我真实遇到的问题。',
        '目前我已经在四台电脑上实际使用：两台 Linux、一台 macOS、一台 Windows。Windows 试得比较少，但在 Linux 和 macOS 上已经连续用了 5 天，我再也没有清理过 <code>target</code> 目录，磁盘也再没有占满过，如下面的截图。从这个结果来看，<code>target</code> 目录占用的问题已经解决了。',
      ],
      shotCaption: `2026 年 11 月 4 日，我在 Omarchy 上的主目录，用 <a href="${disktree}">Disktree</a> 查看。`,
      after: [
        '剩下的就是编译速度，相比 Go 这类语言还是会慢一些。但我们慢一点，用户用起来更快，相当于我们先辛苦点，把好的结果留给用户。而且这个 fork 在这方面也有帮助：不再需要删掉整个 <code>target</code> 目录，就不会从头编译，之前编译的结果都还留着可以复用，编译其实还更快了。',
      ],
      postsTitle: '这一路我在 X 上的记录',
      more: (home: string) => `看看<a href="${home}#changes">有哪些改动</a>和<a href="${home}#measured">实测数据</a>。`,
    },
  },
} as const;
