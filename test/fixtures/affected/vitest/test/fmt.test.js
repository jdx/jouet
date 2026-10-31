import { expect, test } from 'vitest'
import { shout } from '../src/fmt.js'

test('shout', () => expect(shout('hi')).toBe('hi!'))
