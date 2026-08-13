# frozen_string_literal: true

require 'liquid'
require 'minitest/autorun'

class SharedTest < Minitest::Test
  TEMPLATE = Liquid::Template.parse(
    File.read(File.expand_path('../src/shared.liquid', __dir__))
  )

  def test_uses_production_custom_field_for_assets
    output = render(
      'trmnl' => {
        'plugin_settings' => {
          'custom_fields_values' => {
            'public_base_url' => 'https://example.github.io/newspaper/'
          }
        }
      }
    )

    assert_includes output,
                    'href="https://example.github.io/newspaper/assets/berlin-times.css"'
  end

  def test_local_stylesheet_override_wins
    output = render(
      'stylesheet_url' => 'http://host.docker.internal:8000/assets/berlin-times.css'
    )

    assert_includes output,
                    'href="http://host.docker.internal:8000/assets/berlin-times.css"'
  end

  def test_missing_custom_field_uses_canonical_pages_site
    output = render

    assert_includes output,
                    'href="https://pr0me.github.io/trmnl-apps/assets/berlin-times.css"'
  end

  private

  def render(assigns = {})
    TEMPLATE.render(
      {
        'lead_image' => {
          'url' => 'https://example.github.io/newspaper/assets/lead.jpg'
        }
      }.merge(assigns)
    )
  end
end
