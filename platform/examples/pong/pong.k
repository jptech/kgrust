// Pong on the virtual board's screen: no RTL, and no frame buffer.
//
// The generated core runs the 640x480 timing and asks `vga_pixel` for each
// visible pixel's colour as the beam reaches it; the picture is a function
// of a few registers, which `vga_frame` updates once a frame, in the
// blanking after the last line. Both bind to the board's `vga` resource by
// their names. The paddles move with the buttons (W S and the arrow keys on
// the front panel), or follow the ball when left alone; the score is on the
// LEDs, the left player's in the high four.
//
//     kg board serve        # then open the front panel it prints

const uint10 WIDTH = 640;
const uint10 HEIGHT = 480;
const uint10 PADDLE = 64;       // a paddle's height
const uint10 BALL = 8;          // the ball's side
const uint10 LEFT_X = 16;       // the paddles' columns, 8 wide
const uint10 RIGHT_X = 616;
const uint10 SPEED = 4;         // the ball, in pixels a frame, each way
const uint10 STEP = 6;          // a paddle under a button
const uint10 FOLLOW = 3;        // a paddle left alone: slower, so it misses

class Pong
{
private:
    uint10 _left;               // the paddles' tops
    uint10 _right;
    uint10 _x;                  // the ball's top left
    uint10 _y;
    bool _rightward;
    bool _downward;
    uint4 _left_score;
    uint4 _right_score;

    //| The board's buttons: bit 0 left paddle up, 1 down; 2 right up, 3 down.
    [[latency(0)]] () -> uint4 _buttons;

    //| The board's row of LEDs.
    [[latency(0)]] (uint8 lit) -> void _leds;

    inline bool within(uint10 v, uint10 from, uint10 size)
    {
        return v >= from && v < from + size;
    }

    // A paddle moved by its buttons, or toward the ball.
    inline uint10 paddle(uint10 top, bool up, bool down, uint10 ball)
    {
        uint10 target = ball > PADDLE / 2 ? ball - PADDLE / 2 : 0;
        uint10 moved = top;
        if (up || down)
        {
            moved = up ? (top > STEP ? top - STEP : 0) : (top < HEIGHT - PADDLE - STEP ? top + STEP : HEIGHT - PADDLE);
        }
        else if (target + FOLLOW < top)
        {
            moved = top - FOLLOW;
        }
        else if (target > top + FOLLOW && top < HEIGHT - PADDLE - FOLLOW)
        {
            moved = top + FOLLOW;
        }
        return moved;
    }

public:
    //| The colour at (x, y), {r, g, b} 4 bits each: white ball and paddles,
    // a grey dashed net, on dark blue.
    [[latency(1)]] uint12 vga_pixel(uint10 x, uint10 y)
    {
        bool ball = within(x, _x, BALL) && within(y, _y, BALL);
        bool paddles = (within(x, LEFT_X, 8) && within(y, _left, PADDLE)) || (within(x, RIGHT_X, 8) && within(y, _right, PADDLE));
        bool net = within(x, WIDTH / 2 - 2, 4) && ((y >> 4) & 1) == 0;
        return (ball || paddles) ? 0xFFF : (net ? 0x666 : 0x013);
    }

    //| Once a frame, after the last visible line: the paddles, the ball,
    // the score.
    [[no_backpressure]] void vga_frame()
    {
        uint4 buttons = _buttons();
        uint8 lit;
        atomic
        {
            _left = paddle(_left, (buttons & 1) != 0, (buttons & 2) != 0, _y);
            _right = paddle(_right, (buttons & 4) != 0, (buttons & 8) != 0, _y);

            // Clamped at the edges, never wrapped: the ball starts at the
            // corner, heading left, so the first frame is a miss and serves.
            uint10 x = _rightward ? _x + SPEED : (_x > SPEED ? _x - SPEED : 0);
            uint10 y = _downward ? _y + SPEED : (_y > SPEED ? _y - SPEED : 0);
            if (y == 0)
            {
                _downward = true;
            }
            if (y >= HEIGHT - BALL)
            {
                y = HEIGHT - BALL;
                _downward = false;
            }

            // A paddle sends the ball back; past one, the other side scores
            // and the ball serves again from the middle.
            bool left_hit = !_rightward && within(x, LEFT_X, 8) && y + BALL > _left && y < _left + PADDLE;
            bool right_hit = _rightward && x + BALL > RIGHT_X && x < RIGHT_X + 8 && y + BALL > _right && y < _right + PADDLE;
            bool left_miss = !_rightward && x == 0;
            bool right_miss = _rightward && x >= WIDTH - BALL;
            if (left_hit)
            {
                _rightward = true;
            }
            if (right_hit)
            {
                _rightward = false;
            }
            if (left_miss || right_miss)
            {
                if (left_miss)
                {
                    _right_score = _right_score + 1;
                }
                else
                {
                    _left_score = _left_score + 1;
                }
                x = WIDTH / 2;
                y = HEIGHT / 2;
                _rightward = !_rightward;
            }
            _x = x;
            _y = y;
            lit = concat(_left_score, _right_score);
        }
        _leds(lit);
    }
}

export Pong;
