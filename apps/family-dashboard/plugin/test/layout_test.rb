# frozen_string_literal: true

require 'selenium-webdriver'

WIDTH = 1872
HEIGHT = 1404
EXPECTED_WEATHER_DAYS = Integer(ENV.fetch('EXPECTED_WEATHER_DAYS', '3'))
EXPECTED_EVENTS = Integer(ENV.fetch('EXPECTED_EVENTS', '5'))
EXPECTED_CITY = Integer(ENV.fetch('EXPECTED_CITY', '2'))
EXPECTED_HOHENSCHOENHAUSEN = Integer(ENV.fetch('EXPECTED_HOHENSCHOENHAUSEN', '2'))

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
  driver.navigate.to(ENV.fetch('LAYOUT_URL', "file://#{path}"))
  layout_ready = driver.execute_async_script(<<~JS)
    const done = arguments[0];
    document.fonts.ready.then(async () => {
      const deadline = Date.now() + 5000;
      while (Date.now() < deadline) {
        const scheduler = window.__terminalizeScheduler;
        const stable = window.FAMILY_DASHBOARD_READY === true &&
          window.TRMNL_PLUGINS_READY !== false && !scheduler?.pending && !scheduler?.inFlight;
        if (stable) {
          await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
          done(true);
          return;
        }
        await new Promise((resolve) => window.setTimeout(resolve, 25));
      }
      done(false);
    }).catch(() => done(false));
  JS

  result = driver.execute_script(<<~JS)
    const box = (selector) => document.querySelector(selector)?.getBoundingClientRect().toJSON();
    const page = document.querySelector('.fd');
    const pageBox = page?.getBoundingClientRect();
    const outside = Array.from(page?.querySelectorAll('*') || [])
      .filter((element) => {
        if (element instanceof SVGElement && element.tagName.toLowerCase() !== 'svg') return false;
        const style = getComputedStyle(element);
        const current = element.getBoundingClientRect();
        if (style.display === 'none' || current.width === 0 || current.height === 0) return false;
        return current.left < pageBox.left - 1 || current.top < pageBox.top - 1 ||
          current.right > pageBox.right + 1 || current.bottom > pageBox.bottom + 1;
      })
      .map((element) => element.className?.baseVal || element.className || element.tagName);
    const directionHeadings = Array.from(document.querySelectorAll('.fd-direction__heading'))
      .map((element) => ({
        text: element.textContent.trim(),
        scrollWidth: element.scrollWidth,
        clientWidth: element.clientWidth,
        scrollHeight: element.scrollHeight,
        clientHeight: element.clientHeight
      }));
    return {
      screen: box('.screen'),
      page: box('.fd'),
      weather: box('.fd-weather'),
      right: box('.fd-right'),
      today: box('.fd-today'),
      forecast: box('.fd-forecast'),
      events: box('.fd-events'),
      transit: box('.fd-transit'),
      weatherDays: document.querySelectorAll('.fd-day').length,
      eventsCount: document.querySelectorAll('.fd-event').length,
      city: document.querySelectorAll('[data-direction="city"] .fd-departure').length,
      hohenschoenhausen: document.querySelectorAll('[data-direction="hohenschoenhausen"] .fd-departure').length,
      directionHeadings,
      outside
    };
  JS

  failures = []
  failures << 'plugin layout did not reach a stable state' unless layout_ready
  failures << "screen width is #{result.dig('screen', 'width')}" unless result.dig('screen', 'width').to_f.round == WIDTH
  failures << "screen height is #{result.dig('screen', 'height')}" unless result.dig('screen', 'height').to_f.round == HEIGHT
  failures << "page width is #{result.dig('page', 'width')}" unless result.dig('page', 'width').to_f.round == WIDTH - 20
  failures << "page height is #{result.dig('page', 'height')}" unless result.dig('page', 'height').to_f.round == HEIGHT - 20
  failures << "expected #{EXPECTED_WEATHER_DAYS} forecast days, received #{result['weatherDays']}" unless result['weatherDays'] == EXPECTED_WEATHER_DAYS
  failures << "expected #{EXPECTED_EVENTS} events, received #{result['eventsCount']}" unless result['eventsCount'] == EXPECTED_EVENTS
  failures << "expected #{EXPECTED_CITY} city departures, received #{result['city']}" unless result['city'] == EXPECTED_CITY
  unless result['hohenschoenhausen'] == EXPECTED_HOHENSCHOENHAUSEN
    failures << "expected #{EXPECTED_HOHENSCHOENHAUSEN} Hohenschönhausen departures, received #{result['hohenschoenhausen']}"
  end

  columns = result.dig('weather', 'width').to_f / (result.dig('weather', 'width').to_f + result.dig('right', 'width').to_f)
  failures << format('weather column occupies %.2f%%', columns * 100) unless columns.between?(0.52, 0.56)
  weather_rows = result.dig('today', 'height').to_f / (result.dig('today', 'height').to_f + result.dig('forecast', 'height').to_f)
  failures << format('today occupies %.2f%% of weather', weather_rows * 100) unless weather_rows.between?(0.32, 0.35)
  right_rows = result.dig('events', 'height').to_f / (result.dig('events', 'height').to_f + result.dig('transit', 'height').to_f)
  failures << format('events occupy %.2f%% of right column', right_rows * 100) unless right_rows.between?(0.56, 0.60)

  clipped_headings = result['directionHeadings'].select do |heading|
    heading['scrollWidth'] > heading['clientWidth'] + 1 || heading['scrollHeight'] > heading['clientHeight'] + 1
  end
  unless clipped_headings.empty?
    details = clipped_headings.map do |heading|
      "#{heading['text']} #{heading['scrollWidth']}/#{heading['clientWidth']}x#{heading['scrollHeight']}/#{heading['clientHeight']}"
    end
    failures << "direction headings clip: #{details.join(', ')}"
  end
  failures << "elements exceed page: #{result['outside'].join(', ')}" unless result['outside'].empty?

  abort(failures.join("\n")) unless failures.empty?
  output = File.expand_path('../_build/layout.png', __dir__)
  driver.save_screenshot(output)
  File.chmod(0o644, output)
  puts format(
    'layout valid: %d weather days, %d events, %d/%d departures',
    result['weatherDays'],
    result['eventsCount'],
    result['city'],
    result['hohenschoenhausen']
  )
ensure
  driver.quit
end
