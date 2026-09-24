Berikut adalah **Script Take Video Tutorial** lengkap dalam bahasa Inggris yang sederhana, profesional, dan mudah dipahami. 

Script ini dirancang dalam format **dua kolom (Visual Action vs. Voiceover Narration)** agar sangat praktis saat proses *screen recording* dan *voice recording*. Penjelasan berfokus penuh pada **keunggulan dan fitur Tabular secara objektif tanpa menyebut atau membandingkan secara negatif aplikasi lain**.

---

# 🎬 Video Production Script: Introducing Tabular

* **Target Duration:** ~5–6 minutes
* **Tone:** Friendly, clear, modern, and professional
* **Language:** Simple, conversational English (Clear pronunciation, accessible vocabulary)

---

## Scene 1: Introduction & First Impression (0:00 – 0:40)

| Time | Visual / Screen Action | Voiceover (Spoken Audio) |
| :--- | :--- | :--- |
| **0:00** | **[Visual]** Title Card animation: *"Tabular: Fast, Native SQL & NoSQL Client"*. Background shows a clean desktop. | "Hi everyone! Welcome to this tutorial. Today, we are exploring **Tabular**—a modern, fast, and native database client built from the ground up in Rust." |
| **0:12** | **[Screen Action]** Click the Tabular icon on desktop/dock. The app opens instantly. Show a stopwatch overlay showing `~0.1s`. | "The first thing you’ll notice is speed. Because Tabular is a native desktop application, it opens virtually in the blink of an eye." |
| **0:24** | **[Screen Action]** Briefly show Task Manager / Activity Monitor highlighting Tabular's low RAM consumption (~40 MB). | "It has a remarkably small memory footprint, which means it stays smooth and responsive, even when running on a laptop with limited battery and resources." |
| **0:33** | **[Screen Action]** Full view of the Tabular UI: clean sidebar, modern dark theme, and tab bar. | "Let’s dive inside and see how Tabular makes your daily database work faster and simpler." |

---

## Scene 2: Unified Multi-Database Support (0:40 – 1:15)

| Time | Visual / Screen Action | Voiceover (Spoken Audio) |
| :--- | :--- | :--- |
| **0:40** | **[Screen Action]** Click `+ New Connection`. Open the connection modal showing database logos. | "One of Tabular’s greatest strengths is versatility. You can connect to both **relational** and **NoSQL** databases under one single interface." |
| **0:52** | **[Screen Action]** Hover over supported engines: PostgreSQL, MySQL, SQLite, Microsoft SQL Server, Redis, and MongoDB. | "Tabular natively supports PostgreSQL, MySQL and MariaDB, SQLite, Microsoft SQL Server, Redis, and MongoDB." |
| **1:03** | **[Screen Action]** Expand a Redis connection in the sidebar. Show the Redis Key Browser with key types (String, Hash, List) and TTL. | "For instance, with Redis, you even get a dedicated visual key explorer with instant type filtering and memory metrics. Everything is unified in one smooth workflow." |

---

## Scene 3: The Modern SQL Editor & IntelliSense (1:15 – 2:20)

| Time | Visual / Screen Action | Voiceover (Spoken Audio) |
| :--- | :--- | :--- |
| **1:15** | **[Screen Action]** Open a new SQL tab. Begin typing a query: `SELECT * FROM users u JOIN orders o ON ...` | "Now let's check out the SQL editor. Tabular is designed with high developer ergonomics." |
| **1:25** | **[Screen Action]** As you type `u.`, the autocomplete popup immediately shows columns belonging strictly to `users`. | "Notice the **IntelliSense**: it automatically understands your table aliases. When you type `u dot`, Tabular instantly knows to suggest columns only from the `users` table." |
| **1:38** | **[Screen Action]** Type `JOIN orders o ON `. The autocomplete popup places the foreign key join condition (`orders.user_id = users.id`) at the top with a star/icon. Hit Enter. | "Even better, when writing a `JOIN`, Tabular checks table relationships and suggests the exact foreign key match right at the top. You don't have to look up primary keys manually." |
| **1:52** | **[Screen Action]** Place the cursor inside a query among multiple statements. Press `Cmd + Enter` (or `Ctrl + Enter`). Show the statement executing smoothly. | "To execute, you don't even need to highlight text. Just place your cursor anywhere inside a query and press `Command + Enter` or `Control + Enter`. Tabular's smart statement parser detects the exact query boundary." |
| **2:06** | **[Screen Action]** Press `Cmd + Shift + F` (format query). Press `Cmd + /` to toggle comments. Move a line up/down using `Alt + Up/Down`. | "You also have handy keyboard shortcuts: format your SQL instantly with `Cmd + Shift + F`, toggle line comments with `Cmd + Slash`, and move lines easily with `Alt + Up and Down`." |

---

## Scene 4: Interactive Data Grid & Cell Inspector (2:20 – 3:10)

| Time | Visual / Screen Action | Voiceover (Spoken Audio) |
| :--- | :--- | :--- |
| **2:20** | **[Screen Action]** Look at the results table. Hover over the Filter bar at the top of the grid. | "Next, let’s inspect our data. Tabular comes with a powerful **Server-Side Filter Builder**." |
| **2:28** | **[Screen Action]** Click `Add Filter` → select `status = 'active'` → click Apply. The grid refreshes. | "Instead of manually typing WHERE clauses, you can build dynamic filters with dropdowns. Tabular sends the optimized condition straight to the database server." |
| **2:40** | **[Screen Action]** In the grid, hover over a column with a Foreign Key. The value looks like a blue hyperlink. Click it. Tabular immediately opens or navigates to the referenced row in the parent table. | "Notice this foreign key column? In Tabular, foreign keys are clickable hyperlinks! Click any foreign key value, and you instantly jump to the corresponding parent record." |
| **2:54** | **[Screen Action]** Double-click a cell containing JSON data. The **Cell Inspector** modal opens. Click between `JSON Tree`, `Raw`, and `Image` tabs. | "For complex cell contents, open the **Multi-Tab Value Inspector**. It formats JSON with a collapsible tree, previews images, and even provides a Hex viewer for binary data." |

---

## Scene 5: Visual Query Profiler (Explain Analyze) (3:10 – 3:50)

| Time | Visual / Screen Action | Voiceover (Spoken Audio) |
| :--- | :--- | :--- |
| **3:10** | **[Screen Action]** Click the `Visual Explain` or `Profile` button on a complex query. The interactive node graph appears. | "Have you ever struggled to read long, raw JSON execution plans? Tabular includes a built-in **Visual Query Profiler**." |
| **3:22** | **[Screen Action]** Smoothly zoom in, pan around the graph, and click on an expensive node highlighted in soft red. | "It transforms `EXPLAIN ANALYZE` outputs from PostgreSQL, MySQL, or SQL Server into a clean, interactive hierarchical tree." |
| **3:34** | **[Screen Action]** Point out the bottleneck tag (e.g., *'Sequential Scan: 72% cost'*). | "Nodes are color-coded by execution cost, and Tabular automatically highlights bottlenecks—like full table scans or disk spills—so you know exactly what to optimize." |

---

## Scene 6: DBA Tools & Native Backup/Restore (3:50 – 4:35)

| Time | Visual / Screen Action | Voiceover (Spoken Audio) |
| :--- | :--- | :--- |
| **3:50** | **[Screen Action]** Right-click a database in the sidebar and select **Backup Database...**. The wizard pops up. | "For database administrators and developers, Tabular includes native management wizards." |
| **4:00** | **[Screen Action]** Show options: compression (`.sql.gz`), target location, and start backup. A real-time progress bar streams the bytes. | "The native Backup and Restore wizard streams progress in real time with built-in compression. It runs in the background without freezing the app." |
| **4:12** | **[Screen Action]** Open the **Processlist / DBA Monitor** tab. Show running queries and active locks. | "You also have a live **Processlist and Deadlock Monitor**. You can inspect active transactions, detect blocked queries, and safely cancel any long-running process with one click." |
| **4:24** | **[Screen Action]** Switch to the built-in **HTTP Client** tab. Show a simple GET request and formatted JSON response. | "Plus, there is a built-in HTTP client right inside Tabular for quick API testing alongside your database queries." |

---

## Scene 7: Built-in AI Assistant & AI Agent Integration (4:35 – 5:25)

| Time | Visual / Screen Action | Voiceover (Spoken Audio) |
| :--- | :--- | :--- |
| **4:35** | **[Screen Action]** Press `Cmd + Shift + A` (or click the AI Assistant button in the top right). The AI chat panel smoothly slides open. | "One of the most exciting capabilities in Tabular is its intelligent AI integration." |
| **4:45** | **[Screen Action]** Type in plain English: *"Show total revenue grouped by month for 2024"*. The AI streams SQL back into the editor with schema awareness. | "Press `Cmd + Shift + A` to open the AI Assistant. It is schema-aware, meaning it understands your tables and columns to generate accurate SQL queries. You can connect it to your preferred model, such as Claude, OpenAI, or local CLI tools." |
| **5:02** | **[Screen Action]** Show the Settings → AI Assistant screen, and briefly mention `tabular mcp`. Show terminal running `tabular mcp`. | "Tabular also features a built-in **Model Context Protocol (MCP)** server. This allows AI coding agents like Claude Code or Cursor to safely inspect your database schema and run read-only queries with complete audit history and zero secret leakage." |

---

## Scene 8: Security, Zero-Knowledge Sync & Summary (5:25 – 6:00)

| Time | Visual / Screen Action | Voiceover (Spoken Audio) |
| :--- | :--- | :--- |
| **5:25** | **[Screen Action]** Open Settings → Sync. Show the encrypted vault badge (*Argon2id + AES-256-GCM*). | "Security is at the heart of Tabular. With its End-to-End Encrypted Cloud Sync, your connection profiles and queries are protected with a Zero-Knowledge Vault. Your credentials are encrypted on your device before they ever touch the network." |
| **5:44** | **[Screen Action]** Zoom out to the full Tabular interface, cleanly displaying pinned tabs, sidebar search, and query results. | "Fast, native, multi-database support, visual query optimization, and modern developer ergonomics—all in one lightweight package." |
| **5:53** | **[Visual]** Outro slide: GitHub download link (`github.com/tabular-id/tabular`), website URL, and community links. | "Download Tabular today from GitHub or the official website and experience database workflows made simple and fast. Thank you for watching!" |

---

# 💡 Creator Tips (Catatan untuk Pengambilan Video)

1. **Kecepatan Bicara (Pacing):**
   * Gunakan tempo sedang (~130–140 kata per menit). Berikan jeda 1 detik saat berpindah fitur agar penonton sempat melihat UI.
2. **Kamera / Screen Zoom:**
   * Saat mendemonstrasikan **IntelliSense alias (`u.`)**, **Foreign Key hyperlink**, dan **Cost Percentage di Query Profiler**, lakukan *smooth zoom-in (120%–140%)* pada bagian kursor/tabel agar detail terlihat jelas di layar smartphone/laptop penonton.
3. **On-Screen Keyboard Badges (Shortcut Overlays):**
   * Gunakan software seperti *KeyCastr* (macOS) atau *Carnac* (Windows) agar tombol `Cmd + Enter`, `Cmd + Shift + F`, `Cmd + P`, dan `Cmd + Shift + A` muncul secara visual di layar.
4. **Data Demo yang Rapi:**
   * Gunakan database sampel yang menarik dan familiar (misalnya skema *E-Commerce*: `users`, `orders`, `order_items`, `products`). Hal ini membuat fitur *Foreign Key hyperlink* dan *Auto-Join suggestion* terlihat sangat memukau secara visual.