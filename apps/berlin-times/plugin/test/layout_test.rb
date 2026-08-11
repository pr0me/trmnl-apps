# frozen_string_literal: true

require 'selenium-webdriver'

WIDTH = 1872
HEIGHT = 1404
MAX_PHOTO_AREA = 0.18
MIN_SUMMARY_FONT_SIZE = 34

options = Selenium::WebDriver::Firefox::Options.new
options.add_argument('--headless')
options.add_argument('--disable-web-security')
driver = Selenium::WebDriver.for(:firefox, options:)

begin
  borders = driver.execute_script(<<~JS)
    return {
      width: window.outerWidth - window.innerWidth,
      height: window.outerHeight - window.innerHeight
    };
  JS
  driver.manage.window.size = Selenium::WebDriver::Dimension.new(
    WIDTH + borders['width'],
    HEIGHT + borders['height']
  )
  Selenium::WebDriver::Wait.new(timeout: 5, interval: 0.1).until do
    driver.execute_script('return [window.innerWidth, window.innerHeight]') == [WIDTH, HEIGHT]
  end
  path = File.expand_path('../_build/full.html', __dir__)
  url = ENV.fetch('LAYOUT_URL', "file://#{path}")
  driver.navigate.to(url)
  layout_ready = driver.execute_async_script(<<~JS)
    const done = arguments[0];
    Promise.all([
      document.fonts.ready,
      ...Array.from(document.images).map((image) => image.complete
        ? Promise.resolve()
        : new Promise((resolve) => {
          image.addEventListener('load', resolve, { once: true });
          image.addEventListener('error', resolve, { once: true });
        }))
    ]).then(async () => {
      const deadline = Date.now() + 5000;
      while (Date.now() < deadline) {
        const scheduler = window.__terminalizeScheduler;
        const stable = window.BERLIN_TIMES_LAYOUT_READY === true &&
          window.TRMNL_PLUGINS_READY !== false &&
          !scheduler?.pending && !scheduler?.inFlight;
        if (stable) break;
        await new Promise((resolve) => window.setTimeout(resolve, 25));
      }
      if (window.BERLIN_TIMES_LAYOUT_READY !== true) {
        done(false);
        return;
      }
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      window.scrollTo(0, 0);
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      done(true);
    }).catch(() => done(false));
  JS

  result = driver.execute_script(<<~JS)
    const screen = document.querySelector('.screen');
    const page = document.querySelector('.bt');
    const images = Array.from(document.querySelectorAll('.bt img'))
      .filter((image) => {
        const style = getComputedStyle(image);
        const box = image.getBoundingClientRect();
        return style.display !== 'none' && image.complete && image.naturalWidth > 0 &&
          box.width > 0 && box.height > 0;
      });
    const photo = images[0]?.getBoundingClientRect();
    const pageBox = page?.getBoundingClientRect();
    const contentBox = document.querySelector('.bt-page')?.getBoundingClientRect().toJSON();
    const mastheadBox = document.querySelector('.bt-masthead')?.getBoundingClientRect().toJSON();
    const datelineBox = document.querySelector('.bt-dateline')?.getBoundingClientRect().toJSON();
    const outside = Array.from(page?.querySelectorAll('*') || [])
      .filter((element) => {
        const box = element.getBoundingClientRect();
        return box.left < pageBox.left - 1 || box.top < pageBox.top - 1 ||
          box.right > pageBox.right + 1 || box.bottom > pageBox.bottom + 1;
      })
      .map((element) => element.className || element.tagName);
    const storyOverflow = Array.from(document.querySelectorAll('.bt-story'))
      .flatMap((story) => {
        const storyBox = story.getBoundingClientRect();
        return Array.from(story.querySelectorAll('*'))
          .filter((element) => {
            const box = element.getBoundingClientRect();
            return box.left < storyBox.left - 1 || box.top < storyBox.top - 1 ||
              box.right > storyBox.right + 1 || box.bottom > storyBox.bottom + 1;
          })
          .map((element) => `${story.dataset.storyId}:${element.className || element.tagName}`);
      });
    const summaries = Array.from(document.querySelectorAll('.bt-summary'));
    const borderedStories = Array.from(document.querySelectorAll('.bt-story'))
      .filter((story) => {
        const style = getComputedStyle(story);
        return ['Top', 'Right', 'Bottom', 'Left']
          .some((side) => Number.parseFloat(style[`border${side}Width`]) > 0);
      })
      .map((story) => story.dataset.storyId);
    return {
      articles: document.querySelectorAll('.bt-story').length,
      headlines: document.querySelectorAll('.bt-headline').length,
      summaries: document.querySelectorAll('.bt-summary').length,
      uniqueStories: new Set(Array.from(document.querySelectorAll('[data-story-id]'))
        .map((story) => story.dataset.storyId)).size,
      visibleImages: images.length,
      caption: document.querySelector('.bt-caption')?.textContent,
      secondaryHeadline: document.querySelectorAll('.bt-headline')[1]?.textContent,
      photoArea: photo ? photo.width * photo.height / (#{WIDTH} * #{HEIGHT}) : 1,
      pageWidth: pageBox?.width,
      pageHeight: pageBox?.height,
      pageScrollTop: page?.scrollTop,
      contentTop: contentBox?.top,
      mastheadTop: mastheadBox?.top,
      mastheadHeight: mastheadBox?.height,
      datelineTop: datelineBox?.top,
      datelineHeight: datelineBox?.height,
      screenWidth: screen?.getBoundingClientRect().width,
      screenHeight: screen?.getBoundingClientRect().height,
      minimumSummaryFontSize: Math.min(...summaries.map((summary) =>
        Number.parseFloat(getComputedStyle(summary).fontSize))),
      clampedSummaries: summaries
        .filter((summary) => getComputedStyle(summary).webkitLineClamp !== 'none')
        .map((summary) => summary.closest('[data-story-id]')?.dataset.storyId),
      clamped: Array.from(document.querySelectorAll('.bt-headline, .bt-summary'))
        .map((element) => {
          const range = document.createRange();
          range.selectNodeContents(element);
          const lineTops = new Set(Array.from(range.getClientRects())
            .filter((box) => box.width > 0 && box.height > 0)
            .map((box) => Math.round(box.top)));
          return {
            element,
            lineCount: lineTops.size,
            lineLimit: Number.parseInt(getComputedStyle(element).webkitLineClamp, 10)
          };
        })
        .filter(({ lineCount, lineLimit }) => Number.isFinite(lineLimit) && lineCount > lineLimit)
        .map(({ element, lineCount, lineLimit }) => ({
          story: element.closest('[data-story-id]')?.dataset.storyId,
          kind: element.className,
          lineCount,
          lineLimit
        })),
      outside,
      storyOverflow,
      borderedStories
    };
  JS

  failures = []
  failures << "plugin layout did not reach a stable state" unless layout_ready
  failures << "expected six articles, received #{result['articles']}" unless result['articles'] == 6
  failures << "expected six headlines, received #{result['headlines']}" unless result['headlines'] == 6
  failures << "expected six summaries, received #{result['summaries']}" unless result['summaries'] == 6
  failures << "story ids are not unique" unless result['uniqueStories'] == 6
  failures << "expected one visible image, received #{result['visibleImages']}" unless result['visibleImages'] == 1
  unless result['caption']&.include?(result['secondaryHeadline'].to_s.strip)
    failures << "caption does not identify photographed secondary story: #{result['caption'].inspect}"
  end
  if result['photoArea'].to_f > MAX_PHOTO_AREA
    failures << format('photo occupies %.2f%% of screen', result['photoArea'].to_f * 100)
  end
  failures << "screen width is #{result['screenWidth']}" unless result['screenWidth'].round == WIDTH
  failures << "screen height is #{result['screenHeight']}" unless result['screenHeight'].round == HEIGHT
  failures << "page width is #{result['pageWidth']}" if result['pageWidth'].to_f <= 0
  failures << "page height is #{result['pageHeight']}" if result['pageHeight'].to_f <= 0
  failures << "page scroll is #{result['pageScrollTop']}" unless result['pageScrollTop'].to_f.round.zero?
  failures << "masthead top is #{result['mastheadTop']}" unless result['mastheadTop'].to_f.round == 10
  failures << "masthead height is #{result['mastheadHeight']}" unless result['mastheadHeight'].to_f.round == 118
  failures << "dateline top is #{result['datelineTop']}" unless result['datelineTop'].to_f.round == 128
  failures << "dateline height is #{result['datelineHeight']}" unless result['datelineHeight'].to_f.round == 42
  failures << "content top is #{result['contentTop']}" unless result['contentTop'].to_f.round == 170
  if result['minimumSummaryFontSize'].to_f < MIN_SUMMARY_FONT_SIZE
    failures << "minimum summary font size is #{result['minimumSummaryFontSize']}"
  end
  unless result['clampedSummaries'].empty?
    failures << "summaries are clamped: #{result['clampedSummaries'].join(', ')}"
  end
  unless result['clamped'].empty?
    details = result['clamped'].map do |clamp|
      "#{clamp['story']} #{clamp['kind']} #{clamp['lineCount']}/#{clamp['lineLimit']} lines"
    end
    failures << "normal fixture clamped: #{details.join(', ')}"
  end
  failures << "elements exceed page: #{result['outside'].join(', ')}" unless result['outside'].empty?
  unless result['storyOverflow'].empty?
    failures << "elements exceed story: #{result['storyOverflow'].join(', ')}"
  end
  unless result['borderedStories'].empty?
    failures << "stories have separator borders: #{result['borderedStories'].join(', ')}"
  end

  abort(failures.join("\n")) unless failures.empty?
  output = File.expand_path('../_build/layout.png', __dir__)
  driver.execute_script('window.scrollTo(0, 0)')
  driver.save_screenshot(output)
  File.chmod(0o644, output)
  puts format('layout valid: six stories, one image, %.2f%% photo area', result['photoArea'].to_f * 100)
ensure
  driver.quit
end
