# frozen_string_literal: true

require 'selenium-webdriver'
require '/app/lib/trmnlp/image_quantizer'

WIDTH = 1872
HEIGHT = 1404
MAX_PHOTO_AREA = 0.18

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
    return {
      articles: document.querySelectorAll('.bt-story').length,
      headlines: document.querySelectorAll('.bt-headline').length,
      summaries: document.querySelectorAll('.bt-summary').length,
      uniqueStories: new Set(Array.from(document.querySelectorAll('[data-story-id]'))
        .map((story) => story.dataset.storyId)).size,
      visibleImages: images.length,
      photoArea: photo ? photo.width * photo.height / (#{WIDTH} * #{HEIGHT}) : 1,
      pageWidth: pageBox?.width,
      pageHeight: pageBox?.height,
      screenWidth: screen?.getBoundingClientRect().width,
      screenHeight: screen?.getBoundingClientRect().height,
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
      storyOverflow
    };
  JS

  failures = []
  failures << "plugin layout did not reach a stable state" unless layout_ready
  failures << "expected six articles, received #{result['articles']}" unless result['articles'] == 6
  failures << "expected six headlines, received #{result['headlines']}" unless result['headlines'] == 6
  failures << "expected six summaries, received #{result['summaries']}" unless result['summaries'] == 6
  failures << "story ids are not unique" unless result['uniqueStories'] == 6
  failures << "expected one visible image, received #{result['visibleImages']}" unless result['visibleImages'] == 1
  if result['photoArea'].to_f > MAX_PHOTO_AREA
    failures << format('photo occupies %.2f%% of screen', result['photoArea'].to_f * 100)
  end
  failures << "screen width is #{result['screenWidth']}" unless result['screenWidth'].round == WIDTH
  failures << "screen height is #{result['screenHeight']}" unless result['screenHeight'].round == HEIGHT
  failures << "page width is #{result['pageWidth']}" if result['pageWidth'].to_f <= 0
  failures << "page height is #{result['pageHeight']}" if result['pageHeight'].to_f <= 0
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

  abort(failures.join("\n")) unless failures.empty?
  output = File.expand_path('../_build/full.png', __dir__)
  driver.execute_script('window.scrollTo(0, 0)')
  driver.save_screenshot(output)
  image = MiniMagick::Image.open(output)
  image.background('white')
  image.alpha('remove')
  image.alpha('off')
  image.write(output)
  TRMNLP::ImageQuantizer.new(depth: 4).call(output)
  File.chmod(0o644, output)
  puts format('layout valid: six stories, one image, %.2f%% photo area', result['photoArea'].to_f * 100)
ensure
  driver.quit
end
